# S10 engine shells: versions, provenance, checksums, tier flags

All shells were downloaded with `curl` through the configured proxy into their own new, empty
directory under `S10/dl/`, verified, then extracted (`unzip`) into their own new, empty directory
under `S10/tools/`, and run by absolute path from `/tmp` (never from inside the download or tool
directory). `run_engines.sh fetch` repeats exactly this with the pinned SHA-256 values below.

## JavaScriptCore (WebKit) — `jsc`

| item | value |
|---|---|
| source | `https://webkitgtk.org/jsc-built-products/x86_64/release/327409@main.zip` (listed as `LAST-IS` on 2026-11-20; the server keeps ~1 month of builds, so a later re-run must use the cached zip in `dl/jsc_327409/`) |
| bundle README | WebKit Platform GTK, Configuration Release, **WebKit Revision 327409@main** (trunk), builder `GTK-Linux-64bit-Release-Packaging-Nightly`, builder date 2026-11-20T02:41:33, builder OS Ubuntu 26.04.1 LTS |
| zip sha256 | `c21a415deebd6e3eb867037d4ca290b9dd21bec12cc9c71f13403da7d8d042f0` (34 718 105 B) = the server's `327409@main.sha256sum` (`sha256sum -c`: OK). Integrity only: the checksum file comes from the same server, WebKit publishes no signature for these bundles. |
| launcher `jsc` sha256 | `e159c7ab3386075429bfa57cce69db5de3fbe49c8a11b77e8a97479f05efea1e` (static wrapper; runs `bin/jsc` with the bundled `ld-linux` and `lib/`) |
| `bin/jsc` sha256 | `caa298bf468f755f5b2849d1d882aaf4969bba0ddb93525afc63a7d1b58ac4a8` |
| `lib/libjavascriptcoregtk-6.0.so.1.9.0` sha256 | `d629b1c23412b7d7b236989583486fe04bf8ad74e9db758065d2a24ae4533a4d` |
| aarch64 build | **none exists**: `webkitgtk.org/jsc-built-products/`, `webkitgtk.org/built-products/` and `wpewebkit.org/built-products/` list only `x86_64/` (checked 2026-11-20). So JSC on ARM (all iPhones, Apple Silicon Macs) is **not** measurable here. |
| how it relates to shipping Safari | trunk revision of 2026-11-20, i.e. newer than every shipping Safari (Safari 26.x and the later 27.x line are cut from earlier WebKit branches; the exact WebKit build numbers of shipping releases need S5/web). Same JavaScriptCore source and the **same Wasm tiers as Safari: IPInt (in-place interpreter, the default first tier; Safari 26 introduced it per S5) → BBQ (baseline JIT) → OMG (optimising JIT, B3/Air)**. Different from Safari on macOS/iOS: the GTK port (Linux x86_64, glibc, Linux signal handling, no Apple JIT entitlements, no JIT cage/APRR, no iOS memory/jetsam policy), the shell's own stack settings (`maxPerThreadStackUsage` 5 MiB on an 8 MiB thread; Safari's real budgets on the WebContent main thread are platform-specific, see RESULTS §6), and no Apple Silicon codegen. |

JSC tier settings used (discovered with `jsc --options`, verified by counting `Generated BBQ`/`Generated OMG`
lines from `--dumpBBQDisassembly=1 --dumpOMGDisassembly=1` on the CI fixture run, `logs/` in RESULTS §2):

| name | flags | what runs (fixture, 10 000 ticks) |
|---|---|---|
| default | (none) | IPInt first, BBQ after warm-up, OMG for hot functions: 465 BBQ + 18 OMG compilations |
| ipint | `--useBBQJIT=0 --useOMGJIT=0` | in-place interpreter only: 0 BBQ, 0 OMG |
| bbq | `--useWasmIPInt=0 --useOMGJIT=0` | every function BBQ-compiled before it runs (1 121), never OMG |
| omg | `--useWasmIPInt=0 --thresholdForOMGOptimizeAfterWarmUp=0 --thresholdForOMGOptimizeSoon=0` | BBQ for the first calls, then OMG at once (1 120 BBQ, 1 238 OMG incl. OSR entries): the most OMG the shell allows (`--useWasmIPInt=0 --useBBQJIT=0` is refused: "INCOHERENT OPTIONS: at least one of useWasmIPInt, or useBBQJIT must be true"; `--useBBQJIT=0` + OMG thresholds 0 compiles nothing: IPInt cannot tier to OMG directly) |
| nojit | `--useJIT=0` | no JIT at all (JS and Wasm): IPInt only, i.e. Safari/WKWebView without JIT (e.g. iOS embedders without the JIT entitlement). Wasm stays available, SIMD included (`useWasmIPIntSIMD=true`) |

Other JSC options that matter here: `maxPerThreadStackUsage=5242880`, `softReservedZoneSize=131072`,
`reservedZoneSize=65536` (stack; RESULTS §6), `disallowMixedWasmExceptions=true` (a module that mixes legacy
`try/catch` and `try_table` is a CompileError), `useWasmRelaxedSIMD=true`, `useWasmSIMD=true`,
`useWasmTailCalls=true`, `useWasmMemory64=true`.

## SpiderMonkey (Firefox) — `js` (jsshell)

Release picked by listing `https://archive.mozilla.org/pub/firefox/releases/` on 2026-11-20 (`logs/ff_releases_index.html`):
newest release **160.0.1** (jsshell dated 16-Nov-2026), newest ESR **153.7.0esr** (09-Nov-2026); the older ESR line
140.x also continues (latest 140.17.0esr), not run.

| build | zip sha256 | `js` sha256 | version string |
|---|---|---|---|
| 160.0.1 linux-x86_64 | `b547d9ed0bc564ba4387a32cc09aa7aa490c4bdc32998a807c38d36a8078ad69` | `0c576893ed3cdf90ae2a838e8ebf206ecd2ea749d1d5d1f6593338b0ae5dea74` | `JavaScript-C160.0.1` |
| 160.0.1 linux-aarch64 | `1c2bd0e53de20a391f0ae9f9163f79479c441fe6c464a45ebe3244c7080a4ddb` | `ddc5e60cecb3eb123752b775b3857445b880f03be8a9eb5f46907c7a610d9621` | (same, `getBuildConfiguration().arm64 = true`) |
| 153.7.0esr linux-x86_64 | `b453f9a0cf38a7d5087d51710b9d260effff11d09b7a89282897de7f06440f5c` | `84d0c7a782e76c3746c5dbd219ead4d86379fc33349b8d8ce1463acca2959dd8` | `JavaScript-C153.7.0` |
| 153.7.0esr linux-aarch64 | `b0731ae3f67000a0f8a3c3126de0126274b1aa33dfee4e20055d41b5192612a2` | `2e1f000229620934a56a7f514f93f83a6ca0c47c5abf4e8132ebe93bd9884a8e` | `JavaScript-C153.7.0` |

Each zip equals the `SHA256SUMS` line of its release directory (`sha256sum -c`: OK), and both `SHA256SUMS` files carry a
**good GPG signature** (`gpg --verify SHA256SUMS.asc SHA256SUMS`: "Good signature from Mozilla Software Releases
<Mozilla release-key identity>", subkey `827E 6586 0867 9618 CD34 9F93 678E 455D 7676 7AA3`, primary
`14F2 6682 D091 6CDD 81E3 7B6D 61B7 B526 D98F 0353`; key file `KEY` from the same release directory, keyring in
`tools/gnupg`). These are the standalone SpiderMonkey shells published with Firefox 160.0.1 and Firefox ESR 153.7.0 in their signed
release directories (built by the same release automation from the same release). Release build (`debug: false`, `release_or_beta: true`).
Different from Firefox: the shell's fixed native-stack quota (no `--stack-size` option exists; `ulimit -s` below the
quota turns deep recursion into a SIGSEGV), no content-process sandbox/memory pressure handling, no browser prefs
(e.g. `javascript.options.wasm_*` defaults are the shell's built-in defaults).

The aarch64 shells run under `qemu-aarch64 8.2.2 -L /usr/aarch64-linux-gnu -E LD_LIBRARY_PATH=<shell dir>` (S7's
cross sysroot, installed by S7; the shells need their bundled `libnspr4.so`/`libplc4.so`/`libplds4.so`).

SpiderMonkey tier settings (from `js --help`; `wasmCompileMode()` reports the active set):

| name | flags | `wasmCompileMode()` |
|---|---|---|
| default | (none) | `baseline+ion` (lazy tiering: baseline first, Ion per hot function) |
| baseline | `--wasm-compiler=baseline` | `baseline` (RabaldrMonkey only) |
| ion | `--wasm-compiler=optimizing` | `ion` (Ion only) |

SpiderMonkey has **no Wasm interpreter**: `--wasm-compiler=none` and `--no-jit-backend` give
`wasmIsSupported() = false` and `typeof WebAssembly = "undefined"`, i.e. Firefox without a JIT has no Wasm at all.

## V8 controls (unchanged harnesses of S1, S2/S8, S4)

Node `v22.23.5` (V8 12.4.254.21-node.57) at `/opt/node22/bin/node`, x86_64; the S1, S4 and S8 Node drivers run
unchanged (`S1/harness/run_bindgen.mjs`, `S8/node/run.cjs`, `S8/harness/run_node.cjs`, `S4/node/run_web.mjs`,
`S4/node/run_stack.mjs`, `S8/node/s4/run_s4.mjs`) against the module copies in `S10/mods/`.

## Modules (bytes identical to the V8 runs of S1/S2/S4/S7/S8; `mods.sha256`)

None of the earlier spikes recorded module hashes; identity is by provenance (copied byte-for-byte from the
directories their scripts ran, at the mtimes shown in RESULTS §2) and by re-running their V8 harness on the copies
(the V8 rows of every table: identical results).

| copy in `S10/mods/` | from | sha256 |
|---|---|---|
| `s1_simd/s1_harness_bg.wasm` (+ glue `s1_harness.js`) | `S1/runs/simd/pkg_node` (S1's recommended `+simd128` build) | `854efa521eaccd51e14b012867ee2c543ab23726e0771c421dd3f928e1d79b8e` |
| `s1_base/s1_harness_bg.wasm` | `S1/runs/base/pkg_node` (no simd128; reference only) | `d19cf9bbc348254430ab3b2859ed5a38c274998079554cedf9f84d4c050ec316` |
| `s7_portable_simd/s1_harness_bg.wasm` | `S7/wasm/portable_simd/pkg_node` (S7 patch 0006, `+simd128`) | `92a9b5b79f0dd32785d2a8eceaf75ed51d932fae01e99cc0c0cbb273e4a19d27` |
| `s7_portable_nosimd/s1_harness_bg.wasm` | `S7/wasm/portable_nosimd/pkg_node` (S7 patch 0006, no simd128) | `88cb5482e0028f5483f1482bb07745977f87f21851571ac3930730ca44658d84` |
| `s8_v2/pkg_node/s2_harness_bg.wasm` (+ native `s8_v2/s2`) | `S8/runs/bin/v2` (KHZ 29-37, legacy EH) | `91b8916a9057fdcd6197835fb59e5a9f1e4ea1c015e6f3a40d2a2c4c5db55454` (native `s2`: `2a1f4bc66c72023d26fdc1eb54581173b9a7193814da0f86da84a1c7227d3de9`) |
| `s4_pkg/s4_rules_bg.wasm` (+ `s4_khz_rules.js`) | `S4/pkg` (wasmi 2.0.2 + instrumentation, `--target web`) | `c95ee18fc37022d0027e503b125b7da219a9276cc6928e8b9577598a9ce286ce` |
| `s8_s4/pkg/s4_rules_bg.wasm` (+ native `s8_s4/s4`) | `S8/runs/bin/s4` (S8's copy of S4 with the per-tick cap) | `52c5d7ab6a3138ab4633c3ce12253179ec5429b675e0816a5a20d07f582980f1` |
| `s10_v2_legacy/pkg_node/s2_harness_bg.wasm` | rebuilt here (`harness/build_eh_variants.sh`, S8 v2 knobs, legacy EH) | `1b0993a45f63e126eb2668ff8ce6203f26fbf1f8b9604e07aaa16f0fb5dd1944` (code = S8 v2's; only data segments with source paths differ) |
| `s10_v2_exnref/pkg_node/s2_harness_bg.wasm` | rebuilt here (same, `KHZ_WASM_LEGACY_EH=false`) | `b38f204489b609db2a5d1cf42701ff63c7a9bafcfdc38ff1780ac70eec84548d` |

Tools (read-only from S1): wasm-tools 1.263.0 `52975ee5…dfc8`, wasm-bindgen 0.2.130 `c0b53399…6eeb`
(`S1/tools/bin`); binaryen wasm-opt 132 (`S1/tools/npm`, one experiment only); wasi-sdk 32.0 (`S2/tools/wasi-sdk`,
sha256 `55fc523e…ef93`) for the EH rebuild.
