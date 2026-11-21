# S11 — web-spike-mac-host: macOS as a build host for the web version (RESULTS)

`S` = `${WEB_SPIKES}/S11` (= `${WEB_SPIKES}/S11`).
Base: S9's scratch tree (`S9/repo`, the combined engine module, copied — never edited) with S9's
pins: rustc/cargo 1.97.0 (2d8144b78, LLVM 22.1.6), wasi-sdk 32.0, binaryen version_132,
wasm-bindgen 0.2.130, wasm-tools 1.263.0, Node v22.23.5. Every cargo command: `CARGO_BUILD_JOBS=2`,
`CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/web_s11/<variant>`, `CARGO_INCREMENTAL=0`,
line-tables-only dev/test debug. No macOS binary was executed; Mach-O facts come from reading the
files. Web facts marked "web (Orchestrator fetch)" were fetched by the Orchestrator on 2026-11-21
(URLs given there). Machine shared with nothing else; timings are reported, never asserted.

## 0. Bottom line

* **macOS can be a supported build host, on Apple silicon and on Intel, with the same pins.**
  wasi-sdk 32.0 and binaryen version_132 publish `arm64-macos` and `x86_64-macos` archives (all four
  downloaded, sha256 recorded; binaryen's equal its published `.sha256`; wasi-sdk publishes no
  checksum file for any platform). They carry the same clang (`22.1.0-wasi-sdk`, llvm-project
  `4434dabb69916856b824f68a64b029c67175e532`) and the **same** files the Lua build reads: the
  `wasm32-wasip1` `libc.a` (827 members), `libsetjmp.a` and all headers are byte-identical to the
  Linux package S2 pinned. Every arm64 Mach-O file is signed (linker-signed ad hoc; nothing to
  `codesign`); the x86_64 ones are unsigned (Intel does not require it). No Homebrew or other
  non-system dylib: only `/usr/lib`, `/System` and bundled `@rpath` libraries.
* **Three host-dependent inputs reach the module**, all measured here:
  1. **`$CARGO_HOME`** is embedded 540 times (panic locations of registry crates). Fix: the remap
     recipe (`--remap-path-prefix`, on by default in the rewritten `build_web.sh`). With it, two
     checkouts that differ in checkout dir, `CARGO_HOME` (fresh registry download), `HOME`/`USER`/
     `TMPDIR`, toolchain location, wasi-sdk location and target dir give **byte-identical** modules
     (a_remap = b_remap = `9f137f8a…`); without it they differ (88e06b16… vs af63c29c…).
  2. **The file system's directory order**: lua-src compiles Lua in `fs::read_dir` order
     (`third_party/lua-src/src/lib.rs:315-320` in the real tree, unsorted). The same tree on tmpfs
     gave a different module (`9c03853e…`), still **20/20 Node parity**. ext4's order depends on a
     per-file-system hash seed, APFS's differs again, so this breaks byte identity even between two
     Linux machines. Fix (a request to W08): sort the list (`patches/0001`, tested: §4.4).
  3. **Host-dependent clang codegen**: in wasi-sdk 32's own sysroot, compiler-rt `multc3.c` (fp128
     complex multiply) compiled on an arm64 host (both the arm64-macos and the arm64-linux package)
     has a different stack-slot layout than on x86_64 (5 identical recompiles on Linux reproduce
     the x86_64 layout); the other 1 046 archive members, incl. all of wasi-libc, have the same
     code. So a Mac build may differ in bytes even with 1 and 2 fixed. A stack-slot permutation does
     not change computed values in well-defined code; the parity gate checks that on the Mac.
* **Gate recommendation (firm):** on macOS the hard gates are the build, the module checks and
  **Node per-tick parity against committed goldens**; equality of the module sha256 with the Linux
  job's is **reported, not required**. The first `web-build-macos` run on a pull request is the
  confidence-closing step (expected outputs in §8).
* **Scripts:** every Linux-only construct in the archive is listed (§5.1). Portable rewrites of
  `build_web.sh`, `fetch_tools.sh` (+ `tools.lock`, platform-keyed), `wasm_env.sh`,
  `size_report.sh` and `check_imports.sh` (POSIX sh, hashing via Node → `shasum` → `sha256sum`,
  sizes via Node, no GNU-only flags, `uname`/Rosetta platform selection, refusal elsewhere) pass
  **105/105** checks (ShellCheck `--shell=sh`: 0 findings; 104 runtime checks) under dash, bash `--posix` and busybox sh with a BSD-like PATH and shimmed
  `uname`/`sysctl` (bash 3.2 itself could not be built here: SKIP, covered by the macOS job). On
  Linux they reproduce S9's module **exactly** (`--no-remap`: `88e06b16…`, glue `7b77282c…`) and the
  remapped reference, with 20/20 Node parity (§5.4).
* **Prerequisites on a Mac:** Xcode Command Line Tools (host build scripts and 9 proc-macro crates
  link with `cc`; `rustc` targets `*-apple-darwin` use `linker-flavor: darwin-cc`), rustup with
  1.97.0 + `wasm32-unknown-unknown`, Node 22. Lowest macOS to state: **15** (the oldest version a GitHub runner offers);
  14.0 is the floor of the pinned arm64 binaries (bundled `libedit.0.dylib` minos 14.0) but no
  macOS 14 runner exists any more.

## 1. Assumptions owned by S11 — verdicts

| # | Assumption (brief) | Verdict | Evidence | Reproduce |
|---|---|---|---|---|
| M1 | wasi-sdk 32.0 publishes arm64-macos and x86_64-macos archives | **confirmed** | both HTTP 200, downloaded: 131 307 682 B `383be7fe…b933d`, 133 615 598 B `a36c9ab1…1b1b` (§2.1) | `scripts/download_assets.sh` |
| M2 | binaryen version_132 publishes arm64-macos and x86_64-macos archives with checksums | **confirmed** | `98aad827…59aa` (7 640 117 B), `40c3de90…832e` (9 069 142 B), each = its `.sha256` | same |
| M3 | wasi-sdk publishes an upstream checksum file | **refuted** | `.sha256`, `.sha256sum`, `.sha256.txt`, `SHA256SUMS(.txt)`, `sha256sums.txt`, `checksums.txt` all 404 for every platform (`dl/PROBES.txt`); GitHub's per-asset `digest` field not checked (needs web, §9) | `sh scripts/download_assets.sh probe-only` |
| M4 | The macOS clang is the same release as Linux's | **confirmed** | `22.1.0-wasi-sdk` and commit `4434dabb69916856b824f68a64b029c67175e532` in `libLLVM`/`libclang-cpp` of all three; identical `VERSION` (wasi-libc 2fc32bc81b9f, config f992bcc08219) (§2.4) | `strings` commands §2.4 |
| M5 | Same sysroot and headers on macOS | **confirmed for everything the Lua build uses; changed elsewhere** | x86_64-macos sysroot = Linux (9 802/9 802 files); arm64-macos: 9 723 identical, 79 differ (libc++/libc++abi/`libc.so`/LTO bitcode, all embedding `/Users/runner/...` build paths); `wasm32-wasip1/libc.a`, `libsetjmp.a`, every sysroot header identical; clang resource headers identical (267/272; the 5 `libclang_rt.builtins.a` differ) | `python3 -I scripts/compare_trees.py …` §2.4 |
| M6 | Mach-O arch, minos, deps, signatures acceptable | **confirmed** (arm64: 11.0, 14.0 for `libedit`; x86_64: 10.12/10.15; system + bundled `@rpath` dylibs only; arm64 all linker-signed ad hoc, x86_64 unsigned; Node Developer ID `HX7739G8FX`, hardened runtime) | `results/macho/TABLE.md` (§2.3) | `sh scripts/inspect_macho.sh` |
| M7 | No `codesign` step needed after extraction | **confirmed** | 0 unsigned arm64 Mach-O in any archive (wasi-sdk 25, binaryen 16, wasm-bindgen 3, wasm-tools 1); pins stay on archives | same |
| M8 | wasm-bindgen 0.2.130 / wasm-tools 1.263.0 have macOS release assets | **confirmed** (recorded, not used: from-source recommended, S11-D2) | wasm-bindgen `.sha256sum` equal for both darwin assets; wasm-tools publishes no checksum | §2.5 |
| M9 | Node 22 darwin-arm64/x64 pins | **confirmed** | v22.23.5 `e7131995…3f3d` / `c5338323…9422` = `SHASUMS256.txt` | §2.6 |
| M10 | Rust 1.97.0 hosts aarch64/x86_64-apple-darwin exist | **confirmed** | channel manifest 2026-07-09: rustc, cargo, rust-std available for both; one host-independent `rust-std-1.97.0-wasm32-unknown-unknown` | §2.7 |
| M11 | x86_64-apple-darwin is Tier 2 at 1.97 | **confirmed with caveat** | web (Orchestrator fetch): aarch64-apple-darwin Tier 1 with host tools (macOS 11+), x86_64-apple-darwin Tier 2 with host tools (macOS 10.12+); no demotion version on that page | §2.7 |
| M12 | `cc` adds no host-dependent flag to the wasm32 clang command | **confirmed (source + both hosts' argv)** | cc 1.11.1: Apple flags only for `target.vendor == "apple"`; argv of a and b differ only in paths (§3.1) | `diff results/repro/a_remap.lvm_cmd.txt results/repro/b_remap.lvm_cmd.txt` |
| M13 | Host environment can leak into the wasm32 clang | **confirmed (risk), fixed** | cc 1.11.1 concatenates `CFLAGS`, `TARGET_CFLAGS`, `CFLAGS_wasm32_unknown_unknown`, `CFLAGS_wasm32-unknown-unknown` (`lib.rs:5482-5503`); clang reads `CPATH`, `C_INCLUDE_PATH`, `OBJC_INCLUDE_PATH`, `CCC_OVERRIDE_OPTIONS`, `COMPILER_PATH`, `SOURCE_DATE_EPOCH`; `wasm_env.sh` drops them (tested) | `bash scripts/test_portability.sh quick` (wasm_env rows) |
| M14 | wasm-bindgen output independent of paths, cwd, repetition | **confirmed** | 4/4 output files identical (§3.2) | `sh scripts/tool_determinism.sh` |
| M15 | wasm-opt output independent of repetition and thread count | **confirmed** | default ×2, `BINARYEN_CORES=1`, `=4`: all `88e06b16…` | same |
| M16 | Host paths in the final module | **changed** | 540 × `$CARGO_HOME/registry/src/index.crates.io-1949cf8c6b5b557f/`, 64 × `/rustc/<commit>/` (host-independent), workspace paths relative, no target-dir/checkout path; name/DWARF/producers stripped by wasm-opt | same |
| M17 | Modules are byte-identical across checkout dirs / CARGO_HOME / user | **refuted without remap, confirmed with remap** | a_plain `88e06b16…` ≠ b_plain `af63c29c…`; a_remap = b_remap `9f137f8a…` at every stage (§4.3) | `sh scripts/repro_paths.sh a_plain b_plain a_remap b_remap` |
| M18 | Module independent of the file system's directory order | **refuted; fix proposed and tested** | c_remap (tmpfs order) `9c03853e…` ≠ a_remap, parity 20/20; with the sort patch d_remap (ext4) = e_remap (tmpfs) (§4.4) | `sh scripts/repro_paths.sh c_remap d_remap e_remap` |
| M19 | wasi-sdk clang codegen is host-independent | **refuted for 1 of 1 047 sampled objects** | compiler-rt `multc3.c.obj`: arm64-macos = arm64-linux ≠ x86_64-linux (stack slots 32/96 swapped); 1 046 others same code (§4.2) | `sh scripts/codegen_cmp.sh …` |
| M20 | Byte-identical modules Linux x86_64 vs macOS arm64/x86_64 | **needs Mac** → report-only CI step | inputs 1-2 fixable, 3 is not; §4.5 | first `web-build-macos` run |
| M21 | The archived scripts are portable | **refuted** (11 Linux-only or broken constructs, 3 more checked and fine, §5.1); rewrites **confirmed** | 105 PASS / 0 FAIL / 1 SKIP (bash 3.2) | `bash scripts/test_portability.sh` |
| M22 | Linux behaviour unchanged with the rewrites | **confirmed** | `--no-remap` = S9 `88e06b16…`/`7b77282c…`; tool trees = S2/S9; parity 20/20 (§5.4) | `sh scripts/linux_e2e.sh` |
| M23 | bash 3.2 available for testing here | **refuted** | no Ubuntu package; source build not permitted in this session | — (CI) |
| M24 | Xcode CLT required | **confirmed** | 31 build scripts + 9 proc-macro crates in the wasm graph are host artefacts; `*-apple-darwin` `linker-flavor: darwin-cc`; wasm link is `rust-lld` (§6.3) | `RUSTC_BOOTSTRAP=1 rustc -Z unstable-options --print target-spec-json --target aarch64-apple-darwin` |
| M25 | Native Mac build needed for parity | **refuted** | macOS arm64 native differs from x86_64 until W02 (S7 §4.5: run 44761528119 printed `ebfb6f43c9a0b859` vs `25097c21c2c3adc1`); the web module is compared with committed goldens (§6.7) | S7 §4.5 |
| M26 | curl downloads carry no quarantine attribute | **needs web** (designed for both cases; tested with a shim) | `fetch_tools.sh` removes `com.apple.quarantine` after verification | test_portability rows "quarantine" |
| M27 | GitHub-hosted macOS labels | **confirmed (web, Orchestrator fetch)** | arm64 `macos-15`, `macos-26`/`macos-latest`; Intel `macos-15-intel`, `macos-26-intel`; macOS 14 retired 2026-11-02 (§7) | — |

## 2. Requirement 1 — toolchain assets

### 2.1 Downloads and checksums

Command: `sh scripts/download_assets.sh` (probes every URL incl. checksum-file spellings into
`dl/PROBES.txt`, downloads each asset into its own new directory `dl/<name>/`, records
`dl/SHA256.txt`); macOS archives extracted with `tar -xzf <archive> -C dl/<name>/x` and never run.

| asset | bytes | sha256 | upstream checksum |
|---|---|---|---|
| `wasi-sdk-32.0-arm64-macos.tar.gz` | 131 307 682 | `383be7fe00ae04691e1859164fd99970a528c55d624b886ed59f7933898b933d` | none published (404 on all spellings) |
| `wasi-sdk-32.0-x86_64-macos.tar.gz` | 133 615 598 | `a36c9ab106a40abe8a051279c233c6bab719a5788ecc0788e8d141796eb55b1b` | none |
| `wasi-sdk-32.0-x86_64-linux.tar.gz` (re-downloaded) | 143 157 058 | `55fc523ebfbc98f69d1034fcfcb83d1ff5610cd9ab7eceef6cd097a30ba4ef93` (= S2 pin) | none |
| `wasi-sdk-32.0-arm64-linux.tar.gz` (comparison only) | 142 940 986 | `b2070865e6cb0c1e97a38e6ac8d9c37a9dfcd0752764ebabc6bacd3e60cedb96` | none |
| `binaryen-version_132-arm64-macos.tar.gz` | 7 640 117 | `98aad827847af7ef990ed7098d885725c8e5b5aae75073403635617ae4e259aa` | `.sha256` equal |
| `binaryen-version_132-x86_64-macos.tar.gz` | 9 069 142 | `40c3de90bb3766bd0282a895e139a6f50253dba49b4f5bb89e66faca162d832e` | `.sha256` equal |
| `binaryen-version_132-x86_64-linux.tar.gz` (re-downloaded) | 117 057 408 | `195ddc94f9bc89f45abdabb0b9eea86023d727ba90eac8b35b80f2544fc30572` (= S9 pin) | `.sha256` equal |

The macOS binaryen archives are 15× smaller than Linux's: `wasm-opt` links
`@rpath/libbinaryen.dylib` (`lib/`, rpath `@loader_path/../lib`) instead of being static, so
`fetch_tools.sh` keeps the whole archive tree (it never copies `bin/` alone). Tarballs carry no pax
or `LIBARCHIVE.xattr.*` headers (python `tarfile` scan), no case-insensitive path collisions
(`sort -f | uniq -di` on every listing: 0), owner `runner`.

### 2.2 Archive layout

wasi-sdk macOS: `bin/clang -> clang-22`, `bin/llvm-ar` (regular), `ar`/`ranlib -> llvm-ar`,
`wasm-ld -> lld`, `clang.cfg` = `--sysroot=<CFGDIR>/../share/wasi-sysroot` (identical in all
three), `share/wasi-sysroot`, `lib/clang/22`. The x86_64-macos package has no `share/man` and no
bundled `libedit` (its `libLLVM.dylib` links `/usr/lib/libedit.3.dylib`); the arm64 one bundles
`lib/libedit.0.dylib` (minos **14.0**).

### 2.3 Mach-O facts (never executed; `scripts/macho.py`, cross-checked with `llvm-otool-18 -l`)

`sh scripts/inspect_macho.sh` → `results/macho/TABLE.md` (full) and per-asset `*.txt|json`:

| asset | key files | arch | minos / sdk | signature | dylibs outside /usr/lib, /System |
|---|---|---|---|---|---|
| wasi-sdk 32 arm64-macos | `clang-22`, `llvm-ar`, `lld`, `libLLVM.dylib`, `libclang-cpp.dylib` (25 Mach-O) | arm64 | 11.0 / 14.5; **`libedit.0.dylib` 14.0** | all 25 linker-signed ad hoc (CodeDirectory flags 0x20002), no CMS, no team | bundled only: `@rpath/libLLVM.dylib`, `libclang-cpp`, `libedit.0`, `liblldb`, `libxml2.16` |
| wasi-sdk 32 x86_64-macos | same (24) | x86_64 | 10.12 / 14.5 | none (24 unsigned) | bundled only |
| binaryen 132 arm64-macos | `wasm-opt`, `libbinaryen.dylib` (16) | arm64 | 11.0 / 14.5 | 16 linker-signed ad hoc | `@rpath/libbinaryen.dylib` (bundled) |
| binaryen 132 x86_64-macos | same (16) | x86_64 | 10.15 / 14.5 | none | same |
| wasm-bindgen 0.2.130 aarch64-apple-darwin | 3 binaries | arm64 | 11.0 / 26.5 | linker-signed ad hoc | none (`libiconv`, CoreFoundation, libSystem) |
| wasm-bindgen 0.2.130 x86_64-apple-darwin | 3 | x86_64 | 10.12 / 26.5 | none | none |
| wasm-tools 1.263.0 aarch64-macos / x86_64-macos | 1 / 1 | arm64 / x86_64 | 11.0 / 10.12, sdk 26.5 | ad hoc / none | none |
| Node v22.23.5 darwin-arm64 / darwin-x64 | `bin/node` | arm64 / x86_64 | 11.0 / 15.0 | CMS, `Developer ID Application: Node.js Foundation (HX7739G8FX)`, hardened runtime (0x10000) | none |

Notarization: a bare Mach-O in a tarball cannot carry a stapled ticket (stapling applies to
bundles, disk images and installer packages); none of these archives is a bundle, so no ticket is
present. Whether Gatekeeper checks them at all depends on the quarantine attribute (§6.1).

### 2.4 Same compiler, same sysroot?

* Release string: `libLLVM`/`libclang-cpp` of all three packages contain `22.1.0-wasi-sdk` and the
  full commit `4434dabb69916856b824f68a64b029c67175e532`; Linux `clang --version`:
  `clang version 22.1.0-wasi-sdk (https://github.com/llvm/llvm-project 4434dabb69916856b824f68a64b029c67175e532)`.
  `VERSION` files identical (`32.0`, wasi-libc `2fc32bc81b9f`, llvm `4434dabb6991`, config `f992bcc08219`).
* Trees (`python3 -I scripts/compare_trees.py linux=<S2 wasi-sdk> arm64mac=… x64mac=… --sub …`,
  `results/wasi_sdk_tree_compare.txt|json`):

| subtree | Linux | arm64-macos | x86_64-macos | identical in all | differing |
|---|---|---|---|---|---|
| `share/wasi-sysroot` | 9 802 | 9 802 | 9 802 | 9 723 | 79, **arm64-macos only**: libc++/libc++abi/libc++experimental `.a`/`.so`, `libc.so` and the `llvm-lto/` bitcode archives of every wasi target |
| `lib/clang` (resource dir) | 272 | 272 | 272 | 267 | 5: `libclang_rt.builtins.a` (arm64-macos only) |
| `include`, `share/cmake`, `share/misc` | | | | all but libxml2/libedit metadata | — |

  The Lua build reads `include/wasm32-wasip1/**`, `lib/wasm32-wasip1/libc.a` and `libsetjmp.a`
  (lua-src's wasm arm; link libs `setjmp`, `c`): **all identical in all three**. The arm64
  differences carry `/Users/runner/work/wasi-sdk/...` DWARF/`__FILE__` paths where Linux has `/src/...`
  (`strings`, `llvm-dwarfdump-18`): that package's libc++ and compiler-rt were compiled on the macOS
  runner. x86_64-macos's sysroot is byte-identical to Linux's (so it was not compiled by the
  x86_64-macos clang; no evidence about that compiler's output, see §4.2).

### 2.5 wasm-bindgen-cli and wasm-tools on macOS

Recommendation (S11-D2): `cargo install --locked --root target/web-tools/cargo <crate> --version
<pin>` on every host, run by `fetch_tools.sh`; crates.io index checksums `wasm-bindgen-cli 0.2.130`
`c37f6904387d19a319525c74af274c73912f3871c35555aa49f3ea64ebb2fc7a` (rust-version 1.86),
`wasm-tools 1.263.0` `14b8ed798ec13f0e0939dcc403836ba5fd7fb29ff9772e6438a9bcc227bfdc96` (rust-version
1.88). Release assets (recorded, not used):

| asset | sha256 | checksum file |
|---|---|---|
| `wasm-bindgen-0.2.130-aarch64-apple-darwin.tar.gz` | `b90ddb9357f584b1af897d58ad77e4cce2b48fc17e2a967bc249a87f72809444` | `.sha256sum` equal |
| `wasm-bindgen-0.2.130-x86_64-apple-darwin.tar.gz` | `eb209c115b7cb95832423c1aa3f4d06d6c53f63ef74fc4b6fb79ef0b2eea5f10` | equal |
| `wasm-bindgen-0.2.130-x86_64-unknown-linux-musl.tar.gz` | `9687f3bb551b9f66f37ed32a40d7913a00f7962db653054e56857bc306c3227f` | equal |
| `wasm-tools-1.263.0-aarch64-macos.tar.gz` | `b568f04746a699c4068b8d751779b60b501cf2cab6d259a3bc973eb5eeb1f62b` | none (404) |
| `wasm-tools-1.263.0-x86_64-macos.tar.gz` | `909061aba8c069aeed8cb0933e50973a9c8e869bbd6a7819176b49871a338424` | none |
| `wasm-tools-1.263.0-x86_64-linux.tar.gz` | `21c036c1740a0552124c79c6e2170cb61c5b016005671a6d5bc06cf642165431` | none |

### 2.6 Node 22 (nodejs.org `v22.23.5/SHASUMS256.txt`, sha256 of that file `658770bd…b53f`, GPG `.asc` downloaded, signature not checked)

`node-v22.23.5-darwin-arm64.tar.gz` `e7131995759b61a0170b646010d2ad35a5d37b4cb0d471f3e8317eac6f742f3d`
(50 004 868 B), `node-v22.23.5-darwin-x64.tar.gz` `c53383238206b9ae4db23c5d8a63645e2e753bec8ab51a49cc81939aa64e9422`
(51 197 448 B), `node-v22.23.5-linux-x64.tar.gz` `c54118104acc524633b5abe106bbf44a53126ec8b4e5fa105cb92b5d7de8a1a1`
(56 856 114 B); all equal `SHASUMS256.txt`. Recommendation: the nodejs.org build (pkg or tarball;
`fetch_tools.sh --node` fetches the pinned tarball); Homebrew `node@22` also works (version floats
within 22.x; parity does not depend on it, brotli sizes may shift by bytes).

### 2.7 Rust 1.97.0 on macOS (`dl/rust-1.97.0-manifest/channel-rust-1.97.0.toml`, sha256 = its `.sha256`)

`results/rust_manifest_mac.txt`: rustc, cargo, rust-std `available = true` for `aarch64-apple-darwin`
and `x86_64-apple-darwin` (e.g. rustc xz `53e339f1…9f68` / `c3c6f6cd…3014`); one
`rust-std-1.97.0-wasm32-unknown-unknown` for all hosts (`474f926f…4509`), so the precompiled std
in the module is the same file everywhere. rustup-init: `aarch64-apple-darwin` `63752a19…f9f`,
`x86_64-apple-darwin` `bc107f1f…5429` (`rustup-init.sha256`). Tier (web, Orchestrator fetch,
https://doc.rust-lang.org/rustc/platform-support.html): aarch64-apple-darwin Tier 1 with host tools
(macOS 11+); x86_64-apple-darwin Tier 2 with host tools (macOS 10.12+). Tier 2 with host tools
means: official rustup builds, guaranteed to build, but the test suite is not run on it as a
merge gate, so a regression specific to Intel macOS can ship. For this project that is mitigated by
the Intel CI leg (§7).

## 3. Requirement 2 (a)-(c) — what can reach the module from the host

### 3.1 cc 1.11.1 and lua-src (source, plus the argv both hosts' builds printed)

lua-src (vendored, S2 patch 0001 arm; `S9/repo/third_party/lua-src/src/lib.rs`): wasm32 arm at
lines 196-233 adds `--sysroot=$KHZ_WASI_SYSROOT`, `-isystem $SYSROOT/include/wasm32-wasip1`,
`-mllvm -wasm-enable-sjlj -mllvm -wasm-use-legacy-eh=$KHZ_WASM_LEGACY_EH`, `-ffp-contract=off`,
defines; `KHZ_LUA_MATH` (247); `debug = self.debug.unwrap_or(cfg!(debug_assertions))` (256) is the
**build-script's** `cfg!` (release: false → no `-g`, no `LUA_USE_APICHECK`; same on every host);
sources from **unsorted `fs::read_dir`** (`add_files_by_ext`, 375-390; real tree 315-320): host
dependent, §4.4. mlua-sys 0.13.0 calls `lua_src::Build::new().build(Lua54)` (no host logic).

cc 1.11.1 (`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/cc-1.11.1/src/lib.rs`):
* compiler: `CC_wasm32-unknown-unknown`, `CC_wasm32_unknown_unknown`, `TARGET_CC`, `CC` (first set
  wins, `getenv_with_target_prefixes` 5464-5479), exact path used as is (`env_tool` 4518-4536);
  default for wasm32 is `clang` (4329), so the explicit wasi-sdk clang matters (S9 C5).
* default flags (`add_default_flags` 2974+): `-O3` (OPT_LEVEL), `-ffunction-sections
  -fdata-sections`, **no `-fPIC` for wasm32** (3047), `--target=wasm32-unknown-unknown`; `-g` only
  if DEBUG; `-m64` only for x86_64 targets.
* Apple logic is **target**-conditional: `apple_flags` (4103) only `if target.vendor == "apple"` (3438);
  `-arch` (4112), `-m*-version-min` (4116), `-isysroot` + `SDKROOT` (4139-4142), `xcrun`
  (5555, 5608) live there. The only **host** `cfg!(target_os = "macos")` (2651) calls
  `fix_env_for_apple_os` (5510-5518), which acts only when the target os is macos: a no-op here.
  Other host `cfg!`s choose default compiler names (`cfg!(windows)`, solaris) — overridden by `CC_*`.
* flags inherited from `CARGO_ENCODED_RUSTFLAGS` (`flags.rs`): `-Ctarget-feature` is mapped only
  for x86/x86_64 (`target_feature_cc_flag`, 258-282), so `+simd128` does **not** reach the Lua C;
  `--remap-path-prefix` is not translated.
* environment flags: `CFLAGS`, `TARGET_CFLAGS`, `CFLAGS_wasm32_unknown_unknown`,
  `CFLAGS_wasm32-unknown-unknown` are **concatenated** (`envflags` 5482-5503); `CRATE_CC_NO_DEFAULTS`
  drops all defaults (2827); `CC_SHELL_ESCAPED_FLAGS` changes splitting; `RUSTC_WRAPPER` may wrap
  the compiler (sccache: output-neutral).
* archiver: `AR_wasm32…`/`AR`; `llvm-ar cqD` then `sD` (deterministic: zero dates/uids;
  `ZERO_AR_DATE=1` set too). llvm-ar picks the archive format from the first member: a wasm object
  gives `K_GNU` on every host (`ArchiveWriter.cpp:60-74`; the host default `getDefaultKind()` is
  used only for non-object members, `Archive.cpp:980-983`, llvm-project 4434dabb).
* clang itself reads (strings of `libclang-cpp`/`clang-22`): `CPATH`, `C_INCLUDE_PATH`,
  `OBJC_INCLUDE_PATH`, `CCC_OVERRIDE_OPTIONS`, `COMPILER_PATH`, `SOURCE_DATE_EPOCH`, `SDKROOT` and
  `MACOSX_DEPLOYMENT_TARGET` (Darwin targets only), `RC_DEBUG_OPTIONS`; `clang.cfg` sets only
  `--sysroot` (overridden by lua-src's explicit `--sysroot`).

**Every env var that reaches the wasm32 clang command:** `CC_wasm32_unknown_unknown`,
`AR_wasm32_unknown_unknown`, `KHZ_WASI_SYSROOT`, `KHZ_WASM_LEGACY_EH`, `KHZ_LUA_MATH`, `OPT_LEVEL`,
`DEBUG` (cargo), `TARGET`/`HOST` (cargo), the four `CFLAGS*`, `CRATE_CC_NO_DEFAULTS`,
`CC_SHELL_ESCAPED_FLAGS`, `CC_ENABLE_DEBUG_OUTPUT` (log only), `RUSTC_WRAPPER`, plus clang's own
(`CPATH`, `C_INCLUDE_PATH`, `CPLUS_INCLUDE_PATH`, `OBJC_INCLUDE_PATH`, `CCC_OVERRIDE_OPTIONS`,
`COMPILER_PATH`, `SOURCE_DATE_EPOCH`, `TMPDIR`). `wasm_env.sh` unsets or sets all of them.
**Every host path:** the clang binary, `-I <checkout>/third_party/lua-src/lua-5.4.9`, `--sysroot`
and `-isystem` (wasi-sdk location), `-o <target>/…/out/lib/<hash>-x.o` (the hash prefix is cc's
hash of the source dir, so archive member names differ per checkout), the source path. Measured:
the argv of b differs from a's in exactly these paths (`results/repro/{a,b}_remap.lvm_cmd.txt`),
and the objects are not affected (module bytes identical, §4.3): no `-g`, no `__FILE__`/`__DATE__`
use in Lua 5.4.9 + KHZ (`grep -rn __FILE__ third_party/lua-src`: none).

### 3.2 wasm-bindgen and wasm-opt (`sh scripts/tool_determinism.sh` → `results/tool_determinism.txt`, 4 min 52 s)

* wasm-bindgen 0.2.130, input copied to `"in a/x/some name.wasm"` and to `inb/pin_web.wasm`, run from
  two working directories into two output dirs (one with a space): `pin_web_bg.wasm`
  `a67944ad…` ×2, `pin_web.js` `7b77282c…` ×2, both `.d.ts` equal. Output keeps a 3 MB `name`
  section, `producers`, `target_features`; no DWARF (dropped without `--keep-debug`).
* wasm-opt version_132 `-Oz` (S9 flags): default threads twice, `BINARYEN_CORES=1`,
  `BINARYEN_CORES=4`: all four `88e06b16…` (= S9). `--strip-debug` removes `name`,
  `--strip-producers` removes `producers`; the final module has only `target_features`.
* wasm-bindgen import names `__wbg_<name>_<16 hex>` come from wasm-bindgen-macro's hash of
  `CARGO_PKG_NAME`/`VERSION` (std `DefaultHasher`, fixed keys; the `usize` it mixes in is 64-bit on
  all three hosts); the schema hash in `wasm-bindgen-shared/build.rs:53-64` normalises CRLF only
  on Windows (`#[cfg(windows)]`): same value on Linux and macOS.

### 3.3 Paths embedded in the final module (same script)

| module | host paths in data |
|---|---|
| S9 / a_plain | 540 × `/root/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/<crate>/…`; 64 × `/rustc/2d8144b7880597b6e6d3dfd63a9a9efae3f533d3/` |
| a_remap, b_remap | 540 × `/cargo/registry/src/index.crates.io-1949cf8c6b5b557f/…`; 64 × `/rustc/2d81…/` |

Workspace crates appear as relative `crates/<crate>/src/…` (cargo passes workspace-relative paths);
no target-dir or checkout path appears (OUT_DIR includes carry none). The registry directory name
`index.crates.io-1949cf8c6b5b557f` is the same on macOS: cargo's `short_name` = `short_hash` with
`StableHasher` = `StableSipHasher128`, "stable across platforms … doesn't depend on endianness
and bit-width" (cargo c980f4866 `src/cargo/util/hasher.rs:1-5`, `core/source_id.rs:800-806`,
`sources/registry/mod.rs:464-476`). Path dependencies are hashed relative to the workspace root
(`source_id.rs:564-576`), so `-C metadata` does not depend on the checkout location either.
`profile.*.trim-paths` is unstable in cargo 1.97.0 (`feature trim-paths is required`), hence the
`--remap-path-prefix` recipe.

## 4. Requirement 2 (d) — builds in different places, and host codegen

### 4.1 Variants (`sh scripts/repro_paths.sh`, logs `results/repro/<v>.log`, hashes `results/repro/SHA256.txt`)

| variant | checkout | CARGO_HOME | other host differences | remap | Lua source order |
|---|---|---|---|---|---|
| a_plain / a_remap | `S/work/a/kp` (61 chars shorter than b) | `/root/.cargo` (S9's cache) | rustup toolchain, S2's wasi-sdk, target `web_s11/a` | no / yes | ext4 hash order |
| b_plain / b_remap | `S/work/b/Users/macuser/src/kilohertz-pinball-web-checkout` | **new** `…/Users/macuser/.cargo` (fresh download, `--locked`) | `HOME`, `USER=macuser`, `TMPDIR`; hard-linked toolchain at `…/Users/macuser/.rustup/toolchains/1.97.0-x86_64-unknown-linux-gnu` (other sysroot path); wasi-sdk inside the checkout; target `web_s11/b_a_much_longer_target_directory_name` | no / yes | ext4 (same names → same order) |
| c_remap | `S/work/c/kp` on a tmpfs | `/root/.cargo` | target `web_s11/c` | yes | **tmpfs: reverse creation order** |
| d_remap | `S/work/d/kp` + `patches/0001` | `/root/.cargo` | target `web_s11/d` | yes | sorted |
| e_remap | `S/work/c/kp_sorted` on tmpfs + `patches/0001` | `/root/.cargo` | target `web_s11/e` | yes | sorted |

Remap recipe used (rustc, later entries win): `--remap-path-prefix=<checkout>=/khz`,
`<CARGO_HOME>=/cargo`, `<sysroot>/lib/rustlib/src/rust=/rustc/<commit-hash>`,
`<target-dir>=/khz/target`; C side `CFLAGS_wasm32_unknown_unknown='-ffile-prefix-map=<checkout>=/khz
-ffile-prefix-map=<CARGO_HOME>=/cargo -ffile-prefix-map=<target>=/khz/target'`.

### 4.2 Host-dependent clang codegen (no Mac needed: the macOS packages contain objects their own clang compiled)

`sh scripts/codegen_cmp.sh <linux wasi-sdk> <arm64-macos wasi-sdk> results/codegen_cmp` compares
every member of compiler-rt builtins, libc++abi, libc++, libc, libsetjmp (wasm32-wasip1); objects
whose bytes differ are disassembled (`llvm-objdump-18 -d -r`, relocated immediates and addends
masked, debug sections ignored):

| archive (arm64-macos vs x86_64-linux) | members | byte-identical | same code, paths only | code differs |
|---|---|---|---|---|
| `libclang_rt.builtins.a` | 149 | 7 | 141 | **1 (`multc3.c.obj`)** |
| `libc++abi.a` | 17 | 2 | 15 | 0 |
| `libc++.a` | 54 | 1 | 53 | 0 |
| `libc.a` (wasi-libc, built with `wasisdk://v32.0` prefix maps; 827 members, two named `errno.c.obj`, compared as 826 names) | 826 | 826 | 0 | 0 |
| `libsetjmp.a` | 1 | 1 | 0 | 0 |

`multc3.c.obj` (`__multc3`, `long double` = fp128 complex multiply): two 16-byte stack temporaries
swap places (`i32.const 96`/`32`, `i64.load 32`/`96`, …). The arm64-linux package (built in the
same Red Hat GCC 8.5.0 docker image as x86_64-linux, `readelf -p .comment libLLVM.so`) differs from
x86_64-linux in **exactly the same member** and its `multc3` code equals arm64-macos's
(`results/codegen_cmp_arm64linux/summary.txt`: 1 046 byte-identical, 1 differs). Recompiling
`multc3.c` (llvm-project 4434dabb, fetched) with the Linux clang under 7 flag sets, 5 times, from a
192-character `/Users/runner/…` path: always the x86_64 layout. So the clang **host
architecture** (arm64 vs x86_64) changes code for at least this input; the macOS-arm64 clang
behaves like the arm64-linux one on the 1 047 samples. Running the arm64-linux clang under qemu on
the 33 Lua sources would have measured our own inputs; executing that downloaded binary was not
permitted in this session (residual, §9). The same class of effect is possible in rustc's LLVM
(its aarch64-apple-darwin build) for Rust code; not measurable without a Mac.

### 4.3 Different paths, same bytes with the recipe

| variant | cargo `pin_web.wasm` | wasm-bindgen `_bg.wasm` | final `-Oz` module | glue `pin_web.js` |
|---|---|---|---|---|
| S9 (recorded) | — | — | `88e06b166af4781bacd199d136ea8059077fdd191ddadd023b07b1e1ae16e95b` | `7b77282c74b37c04826ce0e8489e12604a570a4ba187ae5c187632c2d05ccae8` |
| a_plain | `a121ebe3…65fb` 19 329 865 B | `a67944ad…f043` | **`88e06b16…e95b` 11 140 164 B (= S9)** | `7b77282c…` |
| b_plain | `e2c9ef02…4e74` 19 395 273 B | `88d90a66…676a` | `af63c29cf3af7aa3b54bd1ed9ff455572068f7286173a373f1f500835bccc759` 11 205 495 B | `7b77282c…` |
| a_remap | `c1c06c53…f03e` 19 326 665 B | `bf597fec…466d` | **`9f137f8aa14a3252a184d8bcbf40566ef1c6aa2cbb7058bc3e424d6055f56a73`** 11 136 922 B | `7b77282c…` |
| b_remap | `c1c06c53…f03e` | `bf597fec…466d` | **`9f137f8a…6a73` (= a_remap)** | `7b77282c…` |

a_plain vs b_plain (`python3 -I scripts/wasm_sections.py`, `results/repro/diff_a_plain_vs_b_plain.txt`):
type/import/function/table/export/element sections identical; data +65 329 B (540 longer
`CARGO_HOME` strings), code +2 B and memory/global/datacount differ only through the shifted
addresses. a = S9 although checkout and target dir differ: those paths never reach the module.
Cargo build time per variant 4 min 24 s-5 min 12 s, wasm-opt 36-60 s.
Node parity (S9's `node/parity_node.mjs`, 3 tables × {bot60, pat60, mb8} + 600 000-tick soak ×
{Lua, rules.wasm}) on a_remap: **20/20 PASS**, every final hash equal to S9's (37 s;
`results/parity/parity_node_a_remap.txt`).

### 4.4 Directory order (c, d, e)

c_remap = a_remap's files and recipe, Lua sources enumerated in tmpfs order
(`lauxlib lfunc lstrlib … ldebug` instead of ext4's `ldebug ldump llex … lauxlib`):

| variant | cargo wasm | final module | parity |
|---|---|---|---|
| a_remap (ext4 order) | `c1c06c53…` | `9f137f8a…` 11 136 922 B | 20/20 |
| c_remap (tmpfs order) | `3b357712ca1d…23e8` | `9c03853e3d725b9d5f895515feb0c5c9959186116df9f7aace2ab8b718f21fa0` 11 136 916 B | **20/20** (`results/parity/parity_node_c_remap.txt`) |
| d_remap (sorted, ext4) | `caf91578…43ee` | `cc636a59f0862d5d32cd3c31d0c21295514a4e8571174281723e8dbd7ffa8d74` 11 136 913 B | **20/20** (`results/parity/parity_node_d_remap.txt`) |
| e_remap (sorted, tmpfs) | `caf91578…43ee` (= d) | **`cc636a59…8d74` (= d)** | — |

c vs a: function and element sections (function order, table) and code/data differ, data 6 B
smaller (alignment) — link order follows the archive member order. Behaviour equal. On ext4 the
order is the htree hash order, seeded per file system at `mkfs`, so two Linux machines can differ as
well; APFS has its own order. Fix: `patches/0001-lua-src-sorted-source-order.patch` (collect,
`sort()`, then add). With it, the ext4 and tmpfs checkouts give the same bytes at every stage
(d_remap = e_remap: cargo `caf91578…`, bindgen `c4d83d02…`, final `cc636a59…`; both compile
`khz_wasm_shim lapi lauxlib … lzio`), and parity stays 20/20. The new reference differs from a_remap
only because the order changed. The real tree has the same unsorted loop
(`third_party/lua-src/src/lib.rs:315-320`), so native link order is file-system dependent too
(layout only).

### 4.5 Gate decision

Byte identity across hosts **cannot be shown without a Mac**, and §4.2 is direct evidence that the
compiler itself can differ on arm64 hosts. Inputs 1 (paths) and 2 (directory order) are removed by
the recipe and the patch; input 3 is not removable by flags. Therefore:

* **Hard gate (every host):** build succeeds; `check_module` (validate, imports, EH encoding,
  relaxed SIMD, size budget); Node per-tick parity equals the committed goldens (`tests/web/goldens`
  or the native run of the same commit on Linux, W26), 20 runs per artifact.
* **Report only:** the macOS module sha256 vs the Linux job's (same commit, same recipe):
  `::notice` when equal, `::warning` with both hashes and the section diff when not.
* Exact flags (in `scripts/web/wasm_env.sh`, **not** `.cargo/config.toml`, which would change the
  guest `rules-wasm` rebuilds and every native build): `CARGO_ENCODED_RUSTFLAGS` =
  `-C target-feature=+simd128 -C link-arg=-zstack-size=4194304
  --remap-path-prefix=<rustc sysroot>/lib/rustlib/src/rust=/rustc/<commit-hash>
  --remap-path-prefix=<workspace>=/khz --remap-path-prefix=<target dir>=/khz/target
  --remap-path-prefix=<CARGO_HOME>=/cargo` (each also with its `pwd -P` spelling when different,
  e.g. macOS `/tmp` → `/private/tmp`), separated by 0x1f so that paths with spaces survive;
  `CC_SHELL_ESCAPED_FLAGS=1`, `CFLAGS_wasm32_unknown_unknown="'-ffile-prefix-map=<workspace>=/khz'
  '-ffile-prefix-map=<target dir>=/khz/target' '-ffile-prefix-map=<CARGO_HOME>=/cargo'"`.
* What the sha256 check compares: `target/web/dist/<artifact>/pkg/pin_web_bg.wasm` and
  `pin_web.js` per artifact, from `SHA256SUMS` written by `build_web.sh`.
* The Mac step that settles it: `web-build-macos` step "Module sha256 vs Linux (report)" (§7).

### 4.6 Every remaining host-dependent input (checked list)

| input | reaches the module? | status |
|---|---|---|
| `$CARGO_HOME` path (registry crates' panic locations) | yes, 540 strings | fixed by the remap (§4.3) |
| checkout path, target dir, `OUT_DIR`, `HOME`, `USER`, `TMPDIR`, toolchain path, wasi-sdk path | no (a = S9; a_remap = b_remap) | — |
| rust-src local path (only if `rust-src` is installed: rustc may print local std paths) | not here (not installed) | remapped to `/rustc/<commit>` anyway |
| directory enumeration order of `lua-5.4.9/*.c` | yes (c_remap) | fixed by the sort patch (d = e) |
| clang's host architecture (LLVM output on arm64 vs x86_64 hosts) | yes for compiler-rt `multc3` | not fixable by flags: report-only sha256, parity gate |
| rustc's LLVM on aarch64/x86_64-apple-darwin | unmeasured (needs Mac) | same class; report-only |
| proc macros (9 crates; host dylibs, not linked into the module) | only through their token output | wasm-bindgen-macro's names: fixed-key SipHash of package name/version + 64-bit `usize` (same on all three 64-bit LE hosts); serde/thiserror/bytemuck/zerocopy derives are pure; the wasm-bindgen glue and import names were identical in all variants |
| build scripts (31) | only through `cargo:` output | host `cfg!` only in libc (emcc name), proc-macro2/thiserror (probe clean-up), wasm-bindgen-shared (CRLF on Windows): none affects wasm32 output; target facts come from `CARGO_CFG_*` |
| cc host vs target logic | no | §3.1 |
| `RUSTFLAGS` text (remap values differ per machine) | no | a_remap = b_remap although every remap value differs (cargo excludes them from `-C metadata`); only the build-script dir hash moved (`mlua-sys-d9abc5…` → `…dc3d67…`) between plain and remap |
| archive member names (cc prefixes object names with a hash of the source dir) | no | b's `f31cfbd4…-lvm.o` vs a's `c9334711…-lvm.o`, same module |
| line endings | no | `.gitattributes`: `* text=auto eol=lf` |
| non-ASCII / case-colliding paths (APFS normalisation, case-insensitivity) | no | 0 non-ASCII and 0 case collisions in 1 989 tracked files |
| Node / brotli version | size numbers only | Node 22.23.5 pinned in CI |

## 5. Requirement 3 — script portability

### 5.1 Linux-only constructs in the archive (`sh scripts/scan_linuxisms.sh` → `results/linuxisms_scan.txt`, then reviewed)

| file:line | construct | on macOS (bash 3.2 `/bin/sh`, BSD userland) | rewrite |
|---|---|---|---|
| S9/build_web.sh:64 | `sha256sum` | not in the macOS base system (`shasum -a 256` is) | `khz_sha256` (Node → shasum → sha256sum) |
| S9/scripts/get_binaryen.sh:10 | `sha256sum -c -` | same | `fetch_tools.sh` compares strings |
| S9/scripts/get_binaryen.sh:6 | hard-coded `x86_64-linux` URL/sha | wrong archive on a Mac | `tools.lock` platform key |
| S9/wasm_env.sh:3 | hard-coded `wasi-sdk-32.0-x86_64-linux` | wrong toolchain | `KHZ_WASI_SDK` from `tools.env` |
| S9/env.sh:6, run_integration.sh:16 | `/home/user/…` literals | absent path | `CARGO_TARGET_DIR` default `<repo>/target` |
| S9/check_imports.sh:13,15 | `grep -E '^\s*…'` (`\s` is a GNU extension) | BSD grep: needs Mac to confirm | `[[:space:]]` |
| S9/check_imports.sh:14,33 | `wc -l <` | BSD `wc` pads with spaces | `| tr -d ' \t'` |
| S9/check_imports.sh:17-31 | `set -e` + heredoc `node` failing ends the script before `FAIL` is printed | — (bug on every host) | `|| rc=$?` |
| S9/scripts/size_report.sh:41-42, run_integration.sh:27 | `python3` | only with Xcode CLT (3.9) on macOS | Node only |
| S9/scripts/conflicts.sh:41 | `env -u` | BSD env supports `-u` | kept (`build_web.sh` uses it for hyphenated names) |
| S9/build_web.sh:30 | `rustc -vV | sed -n '1p;/^LLVM/p'` | fine (POSIX sed) | kept |
| S9/node/pw.cjs:18,29; t_loadtime.cjs:43; t_mobile.cjs:97 | `chrome-headless-shell-linux64`, SwiftShader `VK_ICD_FILENAMES`, `/opt/pw-browsers` | Linux-only browser setup (macOS Chromium uses Metal) | Chromium jobs stay on Linux (W27) |
| S9/web/serve.cjs:31 | `root + path.sep` | `/` on macOS; correct | kept |
| S10/run_engines.sh (whole) | `#!/bin/bash`, `local`, `sha256sum`, `unzip`, `xargs -P`, linux-x86_64 shells | not needed on a Mac (engine shells are Linux binaries; W26 runs them on Linux) | noted only |

No hits for: `sed -i`/`-r`, `stat -c`, `readlink -f`, `realpath`, `date -d`, `tar --wildcards/--transform`,
`cp --parents`, `xargs -r`, `find -printf`, `grep -P`, `sort -V`, `du -b`, `nproc`, `/proc`,
`apt`, `ldd`, `ulimit`, `mktemp` templates, bash-4 features (`declare -A`, `mapfile`, `${x,,}`,
`&>>`, `|&`). Repo: 1 989 files, 0 case-insensitive collisions, longest path 89 chars.

### 5.2 The rule (recommended for `scripts/web/`)

POSIX `sh` (`#!/bin/sh`; works as bash 3.2 in POSIX mode, dash, busybox); no `local`, arrays or
`[[`; hashing through `khz_sha256` (Node, then `shasum -a 256`, then `sha256sum`), sizes through
Node or `wc -c | tr -d ' '`; CPU count `getconf _NPROCESSORS_ONLN`; directories via
`CDPATH= cd -- … && pwd -P` (physical: macOS `/tmp` is `/private/tmp`); every path quoted; flags
with paths passed as `CARGO_ENCODED_RUSTFLAGS`; `env -u` for hyphenated variables; archives via
`curl -fsSL` + `tar -xzf … -C` into `mktemp -d "<dir>/.stage.XXXXXX"`; no GNU-only flags. CI:
ShellCheck `--shell=sh` on `scripts/web/*.sh` (Linux), plus the macOS job runs them with `/bin/sh`.

### 5.3 The rewrites (`S/scripts/web/`) and their tests

`lib.sh` (platform: `Linux/x86_64|amd64` → linux-x86_64, `Darwin/arm64` → darwin-arm64,
`Darwin/x86_64` → darwin-x86_64 unless `sysctl.proc_translated = 1` → darwin-arm64 with a note,
MINGW/MSYS/Cygwin → refused "use WSL2", anything else refused, exit 3), `tools.lock`,
`fetch_tools.sh`, `wasm_env.sh`, `build_web.sh`, `size_report.sh`, `check_imports.sh`.
`bash scripts/test_portability.sh` (final scripts: `results/portability/table.final.md`, log
`logs/test_portability.final.log`; the run before the ShellCheck edits: `table.full.md`, same 104
runtime rows): **105 PASS, 0 FAIL, 1 SKIP** — ShellCheck 0.9.0 `--shell=sh -x` on the six scripts
with 0 findings, then per shell over dash 0.5.12, bash 5.2 in sh
mode and busybox 1.36.1 sh (Ubuntu's dynamic build, which looks commands up on PATH; the static
build prefers its applets so PATH shims cannot replace `uname` there), each running the scripts
itself with a PATH holding only allowed tools: `sha256sum`, `nproc`, `realpath` absent; `readlink
-f`, `stat -c`, `sed -i/-r`, `grep -P`, `xargs -r`, `date -d`, `sort -V`, `du -b`, `find -printf`,
`cp --parents` trapped (trap log empty in every shell). Per shell: 8 platform cases (incl. Rosetta
and the refusals), sha256 via each backend and the automatic fallbacks, size/ncpu, pin selection
for each platform, fetch/verify/extract into a directory with a space, quarantine removal on
Darwin (xattr shim, also when `xattr` fails), cache hit, wrong pin (exit 1, archive deleted,
nothing extracted), `wasm_env.sh` dropping `CFLAGS`/`CPATH`/`RUSTFLAGS` and keeping remap entries
with spaces, `build_web.sh --dry-run` on Linux, refusal of another platform's tools, refusal
without Xcode CLT, dry run on (shimmed) macOS with CLT, size report = S9 §8 (10.62 / 3.93 / 2.79
MiB), budget overrun → exit 1, import audit pass/fail. dash and busybox also fetched the **real
pinned archives for all three platforms** from a local mirror (sha256 OK for wasi-sdk, binaryen and
Node each; Linux smoke OK for clang, llvm-ar, wasm-opt) and the fetched Linux trees equal S2's
wasi-sdk and S9's binaryen file for file. SKIP: bash 3.2 (no package; a source build was not
permitted here) — covered by the macOS job, whose `/bin/sh` is bash 3.2.

### 5.4 Linux behaviour unchanged (`sh scripts/linux_e2e.sh`, under dash)

Run 1 (`results/linux_e2e_run1/`, log `run1.log`), everything under `dash`, `CARGO_HOME=/root/.cargo`:

| step | result |
|---|---|
| `fetch_tools.sh` (mirror of the real archives) | wasi-sdk `55fc523e…` and binaryen `195ddc94…` sha256 OK, extracted; `cargo install --locked wasm-bindgen-cli 0.2.130` 2 min 23 s and `wasm-tools 1.263.0` 3 min 20 s (2 jobs); smoke OK for clang (`22.1.0-wasi-sdk … 4434dabb…`), llvm-ar, wasm-opt 132, wasm-bindgen 0.2.130, wasm-tools 1.263.0 |
| `build_web.sh --no-remap` | **`88e06b166af4781bacd199d136ea8059077fdd191ddadd023b07b1e1ae16e95b` / glue `7b77282c…` = S9's shipped module**; cargo 269 s, wasm-bindgen 1 s, wasm-opt 34 s; sizes 19 329 865 / 15 345 996 / 11 140 164 B (= S9) |
| `build_web.sh` (default remap) | **`9f137f8aa14a3252a184d8bcbf40566ef1c6aa2cbb7058bc3e424d6055f56a73` = repro a_remap/b_remap** (wasm_env.sh orders the remap entries differently from repro_paths.sh: no effect); cargo 266 s, wasm-opt 29 s |
| `check_imports.sh` (both) | `imports=623 non-glue=0 env/wasi=0`, PASS |
| `size_report.sh` (both) | 10.62 / 3.93 / 2.79 MiB (S9 §8) |
| S9's Node parity (both) | **20/20 PASS** each, final hashes = S9's |

The remapped `CARGO_ENCODED_RUSTFLAGS` the script produced (from `dist_remap/build.log`):
`-C target-feature=+simd128 -C link-arg=-zstack-size=4194304
--remap-path-prefix=/root/.rustup/toolchains/stable-x86_64-unknown-linux-gnu/lib/rustlib/src/rust=/rustc/2d8144b7880597b6e6d3dfd63a9a9efae3f533d3
--remap-path-prefix=<S>/work/a/kp=/khz --remap-path-prefix=/home/user/kilohertz-pinball/target/web_s11/e2e_remap=/khz/target
--remap-path-prefix=/root/.cargo=/cargo`.
After run 1 the scripts got ShellCheck-only edits (quoting, directives; `shellcheck --shell=sh -x`:
0 findings, `results/shellcheck_web.txt`); run 2 with the final scripts: `KEEP_TOOLS=1 sh scripts/linux_e2e.sh`
(`results/linux_e2e/`, `run2.log`): fetch = cache hit (`already verified` ×2, smoke OK ×5), plain
**`88e06b16…` / `7b77282c…`** (cargo 281 s), remap **`9f137f8a…`** (cargo 272 s), import audit PASS,
sizes 10.62 / 3.93 / 2.79 MiB within the 3.0 MiB budget, Node parity **20/20** for both.

## 6. Requirement 4 — macOS specifics

### 6.1 (a) Gatekeeper and quarantine

* Whether curl sets `com.apple.quarantine`: **needs web** — the Orchestrator's fetch found no
  source either way. Design for both: `fetch_tools.sh` downloads with curl only, verifies the
  sha256, extracts, then runs `xattr -dr com.apple.quarantine <tool dir>` (errors ignored) on
  Darwin (tested with a shim, incl. a failing `xattr`). The tarballs contain no xattr headers.
* A user who downloads an archive in Safari/Chrome and extracts it by hand gets quarantined
  files; on first run Gatekeeper blocks a quarantined executable that is not notarized (all of
  wasi-sdk, binaryen, wasm-bindgen, wasm-tools: ad hoc or unsigned). Exact dialog text: **needs
  web** (question in §9). Fix: `xattr -dr com.apple.quarantine target/web-tools`, or let
  `fetch_tools.sh` fetch (the documented path); `KHZ_WEB_MIRROR=<dir>` accepts a hand-downloaded
  archive and still checks the pin.
* Node from the nodejs.org pkg/tarball is Developer ID signed (team HX7739G8FX) with hardened
  runtime; notarization status of the tarball binary: needs web.

### 6.2 (b) Signatures

Every executable and dylib of the arm64 archives has `LC_CODE_SIGNATURE` with an ad hoc,
linker-signed CodeDirectory (flags `0x20002`, no CMS blob, no team): enough for the arm64 kernel
rule. x86_64 archives: no signature (24 + 16 + 3 + 1 files), not required on Intel.
**No `codesign --force --sign -` step is needed**; tools.lock pins archives, never files.

### 6.3 (c) Xcode Command Line Tools

Needed. The wasm build compiles **host** artefacts: 31 build scripts in the `pin_web` wasm graph
(`results/build_scripts_in_wasm_graph.txt`) and 9 proc-macro crates (bytemuck_derive,
document-features, gltf-derive, num-derive, serde_derive, thiserror-impl, wasm-bindgen-macro,
zerocopy-derive, zeroize_derive); rustc links them with the system linker (`rustc --print
target-spec-json`: `*-apple-darwin` → `linker-flavor: darwin-cc`). The wasm link itself uses
`rust-lld` from the toolchain (`wasm32-unknown-unknown` → `"linker": "rust-lld"`), and the Lua C
uses wasi-sdk's clang, so CLT's clang never touches the module. Check: `xcode-select -p`
(`build_web.sh` refuses with "run xcode-select --install"; tested with a shim). Full Xcode not needed.

### 6.4 (d) rustup and the pinned toolchain

The repo has no `rust-toolchain.toml` (CI pins `dtolnay/rust-toolchain@1.97.0`, `versions.toml`
`rust = "1.97.0"`). On a Mac: `rustup toolchain install 1.97.0 --profile minimal --target
wasm32-unknown-unknown`, then `rustup override set 1.97.0` in the checkout (or `rustup default
1.97.0`); host `aarch64-apple-darwin` on Apple silicon, `x86_64-apple-darwin` on Intel.
`build_web.sh` refuses any rustc other than 1.97.0 and a missing wasm32 target (tested).

### 6.5 (e) Node 22: §2.6 (nodejs.org v22.23.5 recommended; Homebrew `node@22` acceptable).

### 6.6 (f) Playwright Chromium on macOS (optional local browser tests)

From the installed Playwright 1.63.0 (`/opt/node-tools/node_modules/playwright-core/lib/coreBundle.js`
8497-8516, 32885-32905, `browsers.json`): host platform `mac<N>[-arm64]` (arm64 when the CPU model
contains "Apple"), `isOfficiallySupportedPlatform: macVersion >= 14`; Chromium (Chrome for Testing
153.0.8010.12, revision 1243) and the headless shell exist for mac14/15/26 x64 and arm64, none for
mac ≤ 13. Download sizes (HEAD on cdn.playwright.dev): headless shell arm64 98 831 293 B, x64
104 060 463 B; full Chrome for Testing arm64 190 970 181 B. `npx playwright@1.63.0 install
chromium` (or `chromium-headless-shell`). WebGPU in Chromium on macOS uses Metal; whether
headless Chromium on a GitHub macOS VM exposes a WebGPU adapter: needs web/Mac (W27's job measures
it). Observation for W27: Playwright 1.63.0's `browsers.json` names Chromium 153.0.8010.12 for
revision 1243 while S9 recorded 157.0.8103.78 from the preinstalled shell; pin by Playwright version.

### 6.7 (g) Native Mac build for parity: not needed

S7 §4.5: GitHub run `44761528119` (ci.yml, push of `995935f`), job `replay-hash (aarch64-apple-darwin)`
on `macos-26-arm64/20261117.0516` printed `state_hash=ebfb6f43c9a0b859` and seam digests equal to
qemu-aarch64; x86_64 (Linux, Windows) is `25097c21c2c3adc1`. So **macOS arm64 native does not
equal x86_64 today** (until W02's portable quaternion ops land; with them qemu gives the x86_64
values). The web module equals x86_64 native. Recommendation: the macOS web job compares the
module's Node per-tick hashes with **goldens committed in the repo** (produced by the Linux native
headless run, the same files W26's Linux parity job uses); it builds no native Mac binary.

### 6.8 (h) macOS version floor

| component | floor (evidence) |
|---|---|
| wasi-sdk 32 arm64 | **14.0** (`libedit.0.dylib` minos 14.0, linked by `libLLVM.dylib`; others 11.0) |
| wasi-sdk 32 x86_64 | 10.12 |
| binaryen 132 | 11.0 arm64, 10.15 x86_64 |
| wasm-bindgen / wasm-tools (cargo-built) | the Rust host floor |
| Node v22.23.5 | 11.0 (minos of both binaries) |
| Rust 1.97 host | aarch64 macOS 11+, x86_64 macOS 10.12+ (web, Orchestrator fetch) |
| Playwright 1.63 (optional) | 14 |
| CI images available | 15 and 26 (macOS 14 retired 2026-11-02, web) |

State: **"macOS 15 (Sequoia) or later, Apple silicon or Intel"** (tested); macOS 14 meets every
binary floor but is untested (S11-D5). Whether dyld refuses a dylib whose minos exceeds the OS
(the 14.0 libedit on macOS 13): needs Mac; moot because 13 is below the stated floor.

### 6.9 (i) Browsers on macOS for the local run

No change to the plan's floors (S5/S10): Safari 26+ for the WebGPU artifact, WebGL2 artifact
Safari 16.4+; Chrome/Edge 113+, Firefox 145+ on macOS. The server sends no COOP/COEP and needs
none (S9 Q2); `http://127.0.0.1` and `http://localhost` are secure contexts (S5 §5).
Apple-silicon-specific: U1 (JSC codegen on ARM and the WebContent stack budget) stays with W27.
`serve.cjs` binds 127.0.0.1; with `--host 0.0.0.0` (phones) macOS may ask whether `node` may
accept incoming connections: needs web/Mac (§9).

## 7. Requirement 5 — CI

Labels (web, Orchestrator fetch, https://github.com/actions/runner-images README): arm64
`macos-latest` (= macOS 26), `macos-26`/`macos-26-xlarge`, `macos-15`/`macos-15-xlarge`; Intel
`macos-26-intel`, `macos-26-large`, `macos-latest-large`, `macos-15-intel`, `macos-15-large`; all macOS
14 images retired 2026-11-02 (runner-images issue #15226); `macos-13` no longer listed;
`-large`/`-xlarge` are paid larger runners. Billing (web, Orchestrator fetch, GitHub billing
docs): standard runners free in public repositories; private: macOS $0.062/min vs Linux 2-core
$0.006/min. Whether this repository is public: needs web. Repo today (`grep -c` per file):
`macos-latest` only — ci.yml 4 matrix entries (core-tests-cross, gpu-tests-cross, clippy-cross,
replay-hash-cross; pushes to main and dispatch only, D40), release.yml 3 (verify aarch64, verify
x86_64 cross-built and tested under Rosetta 2, package universal; dispatch/tags), soak.yml 1
(weekly + `pin_app` pushes), spikes.yml 1 (spike paths); no Intel label anywhere, no macOS job on
pull requests today.

**Recommendation — `web-build-macos` in `web.yml`:**

| leg | runs-on | trigger | gate |
|---|---|---|---|
| arm64 | `macos-15` (standard, pinned image: the oldest stated version; `macos-latest` moved to 26 in 2026) | web-path pull requests (the `web-build` path list), push to main, dispatch | yes (within `web.yml`; not in `pr-checks`, Q11) |
| Intel | `macos-26-intel` (standard; macOS 26 expected to be the last Intel macOS: needs web) | push to main on web paths, weekly `schedule`, dispatch | yes when it runs; not on PRs |

Steps: checkout → `dtolnay/rust-toolchain@1.97.0` with `targets: wasm32-unknown-unknown` →
`actions/setup-node` `node-version: 22.23.5` → cache `target/web-tools` keyed by
`runner.os`-`runner.arch`-`hashFiles('scripts/web/tools.lock')` and `Swatinem/rust-cache` →
"Xcode CLT present" (`xcode-select -p`) → `/bin/sh scripts/web/fetch_tools.sh` (smoke checks run
every binary) → `/bin/sh scripts/web/build_web.sh` per artifact → `check_module.sh` → Node parity
vs committed goldens (20 runs per artifact) → `size_report.sh` (budgets) → "Module sha256 vs Linux
(report)": download the Linux `web-build` artifact's `SHA256SUMS`, compare, `::notice`/`::warning`
→ upload `SHA256SUMS` + `build.log`. Asserts: build, module checks, parity (hard); sha256 equality
(report). Estimate (scaled from S9/S11 timings, an estimate): cold 18-25 min (cargo 5-6 min per
artifact, `cargo install` of the two CLIs 4-6 min, tool fetch 1 min, wasm-opt 1 min, parity 1-2 min);
warm (caches) 8-12 min; within the 45-minute job limit. Safari via `safaridriver` stays S10's/W27's
job (not redone).

## 8. The confidence-closing step

The first pull request that adds `web-build-macos` is the verification. Expected outputs on
`macos-15` (arm64) and `macos-26-intel`: `fetch_tools.sh` prints `sha256 OK` with the darwin pins of
`tools.lock` and `smoke OK clang version 22.1.0-wasi-sdk (… 4434dabb…)`, `LLVM version
22.1.0-wasi-sdk`, `wasm-opt version 132 (version_132)`, `wasm-bindgen 0.2.130`, `wasm-tools 1.263.0`;
`build_web.sh` prints `host darwin-arm64 (Darwin arm64)` / `darwin-x86_64`; `check_module` passes
(0 `env`/`wasi_*` imports, legacy EH only, no relaxed SIMD); Node parity 20/20 per artifact equal
to the committed goldens; the sha256 report shows whether the module equals Linux's (equality is
likely for the Intel leg, possible but not expected-with-certainty for arm64, §4.2). A parity
failure on macOS is a real defect (stop and investigate); a sha256 difference alone is not.

## 9. Residual risks and open questions

* Not run (no Mac): every macOS binary; bash 3.2 (`/bin/sh`); Gatekeeper behaviour; APFS order.
* Not permitted in this session: running the arm64-linux wasi-sdk clang under qemu on the 33 Lua
  sources (would have shown whether an arm64-host clang changes our own objects); building bash
  3.2.57 from source (`dl/bash-3.2.57`, sha256 `3fa9daf8…06a4`, downloaded, not built).
* Rust-side host codegen (rustc's LLVM on aarch64-apple-darwin) unmeasured; parity is the guard.
* needs web: (1) does `curl` (macOS `/usr/bin/curl`) ever set `com.apple.quarantine`, and the exact
  Gatekeeper dialog for a quarantined, non-notarized ad hoc / unsigned CLI binary on macOS 15/26;
  (2) GitHub release `digest` (sha256) of the four wasi-sdk 32 assets
  (`api.github.com/repos/WebAssembly/wasi-sdk/releases/tags/wasi-sdk-32`, `assets[].digest`) to
  cross-check the pins, since no checksum file exists; (3) is `<owner>/kilohertz-pinball` public
  (runner billing); (4) announced end date for `macos-15-intel`/`macos-26-intel`; (5) does the
  nodejs.org darwin tarball binary carry a notarization record (`spctl`); (6) macOS firewall prompt
  for `node` binding 0.0.0.0; (7) is macOS 26 the last macOS release for Intel Macs (Apple), i.e.
  how long `macos-26-intel` keeps Intel covered.

## 10. Files

* Reports: `S/RESULTS.md`, `S/PLAN_TEXT.md`, `S/DECISIONS_LOCAL.md`, `S/QUEUE_LINES.md`.
* Experiment scripts (`S/scripts/`): `download_assets.sh`, `inspect_macho.sh` + `macho.py`,
  `compare_trees.py`, `codegen_cmp.sh`, `repro_paths.sh`, `wasm_sections.py`, `tool_determinism.sh`,
  `scan_linuxisms.sh`, `test_portability.sh`, `linux_e2e.sh`.
* Portable rewrites (`S/scripts/web/`): `lib.sh`, `tools.lock`, `fetch_tools.sh`, `wasm_env.sh`,
  `build_web.sh`, `size_report.sh`, `check_imports.sh`.
* Patch: `S/patches/0001-lua-src-sorted-source-order.patch`.
* Results (`S/results/`): `macho/` (TABLE.md, per-asset txt/json, llvm cross-check),
  `wasi_sdk_tree_compare.{txt,json}`, `codegen_cmp/`, `codegen_cmp_arm64linux/`, `repro/` (per-variant
  modules, logs, cc argv, `SHA256.txt`, section diffs), `parity/` (Node parity of a_remap, c_remap,
  d_remap), `tool_determinism.txt`, `portability/table.{final,full}.md`, `linux_e2e/` (run 2),
  `linux_e2e_run1/`, `shellcheck_web.txt`, `rust_manifest_mac.txt`, `linuxisms_scan.txt`,
  `build_scripts_in_wasm_graph.txt`, `wasm_graph_*.txt`, `cargo_metadata_wasm.json`.
* Downloads (`S/dl/<asset>/`, extracted copies in `x/`): `SHA256.txt` (every downloaded file),
  `PROBES.txt` (HTTP status of every probed URL).
* Test tools (`S/tools/bin/`): `busybox` (busybox-static) and `busybox-dyn` (busybox), both Ubuntu
  1:1.36.1-6ubuntu3.1 (debs `944b2728…`, `1b4d228f…`), and `shellcheck` 0.9.0 (deb `eadf78f4…`),
  taken from the apt mirror with `apt-get download` + `dpkg-deb -x`, not installed.
* Logs: `S/logs/` (`repro_paths.run*.log`, `test_portability.{quick,full,final}.log`,
  `linux_e2e.log`, `linux_e2e.run2.log`, `download_assets.log`, `inspect_macho.log`).
* Cleaned at the end: `/home/user/kilohertz-pinball/target/web_s11` (`cargo clean`), the tmpfs
  mount, `work/{tp,e2e-tools,e2e-mirror}` and b's fresh `CARGO_HOME` (each script recreates them).

## 11. Reproduction (each command re-runs to the same hashes; sizes exact; timings vary)

```sh
cd $WEB_SPIKES/S11
sh scripts/download_assets.sh                 # assets, sha256, upstream checksum files, PROBES.txt
sh scripts/inspect_macho.sh                   # Mach-O table
python3 -I scripts/compare_trees.py linux=../S2/tools/wasi-sdk/wasi-sdk-32.0-x86_64-linux \
  arm64mac=dl/wasi-sdk-32-arm64-macos/x/wasi-sdk-32.0-arm64-macos x64mac=dl/wasi-sdk-32-x86_64-macos/x/wasi-sdk-32.0-x86_64-macos \
  --sub share/wasi-sysroot --sub lib/clang
sh scripts/codegen_cmp.sh "$PWD/../S2/tools/wasi-sdk/wasi-sdk-32.0-x86_64-linux" "$PWD/dl/wasi-sdk-32-arm64-macos/x/wasi-sdk-32.0-arm64-macos" "$PWD/results/codegen_cmp"
sh scripts/repro_paths.sh                     # 7 variants, ~6 min each (root: mounts a tmpfs for c/e)
sh scripts/tool_determinism.sh                # needs results/repro/a_plain
bash scripts/test_portability.sh              # 105 PASS, 1 SKIP (quick: skip the real-archive fetches)
sh scripts/linux_e2e.sh                       # S9 bytes with --no-remap, 9f137f8a… with remap, parity 20/20
```
Expected: a_plain/S9 `88e06b16…`; a_remap = b_remap `9f137f8a…`; c_remap `9c03853e…`; d_remap =
e_remap `cc636a59…`; glue `7b77282c…` in every variant; Node parity 20/20.
