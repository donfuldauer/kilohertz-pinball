# S2 (web-spike-lua) — local decisions (PLAN §2.3 rule 8; not in the repo)

## S2-1 — Reference for "native" command streams
**Decision:** Compare against S1's native recordings (`S1/rec/*.scr`, `S1/native/*.hashes`), made with the
unpatched repo's real Lua natively, rather than re-recording with the S2-patched native build.
**Rationale:** The S1 recordings are the untouched engine's output, so a match proves both that the web build
equals native and that the S2 patches (lkhznum, LUAL_BUFFERSIZE, unified math, web-parity) did not change the
three production tables natively. The S2 native build is run against the same files (all PASS).
**Alternatives:** Re-record natively with the S2 build (would hide a native change).

## S2-2 — Route (b) uses wasi-sdk 32 (LLVM 22.1.0) instead of apt clang-18
**Decision:** The working build uses the wasi-sdk 32 tarball (clang 22.1.0 + wasi-libc 2fc32bc81b9f +
libsetjmp), checksum-pinned, for the C side; Rust 1.97.0 (LLVM 22.1.6) links it.
**Rationale:** LLVM >= 19 lowers setjmp to `__wasm_setjmp`/`__wasm_setjmp_test` with a stack buffer; LLVM 18
(Ubuntu clang-18) uses `saveSetjmp`/`testSetjmp`/`getTempRet0` and calls `malloc`/`free` in every
`luaD_rawrunprotected` (measured, route (a)), i.e. a C-heap allocation per protected call outside the Lua arena
and invisible to the D5 guard. wasi-sdk also ships the matching `libsetjmp.a`.
**Alternatives:** apt clang-20 + apt `wasi-libc` (2023 snapshot, no libsetjmp: a hand-written runtime would be
needed); apt clang-18 + hand-written LLVM-18 runtime (works, rejected for the per-pcall malloc).

## S2-3 — Legacy Wasm EH (try/catch) by default
**Decision:** `-mllvm -wasm-use-legacy-eh=true` (lua-src arm default, knob `KHZ_WASM_LEGACY_EH`).
**Rationale:** wasm-bindgen 0.2.130 emits legacy `try/catch_all` in its own export wrappers anyway once the
module contains EH; legacy EH has the longest browser support (exact Safari/Firefox/mobile versions: S5).
Both legacy and exnref run in Chromium 157 and Node 22.23.5.
**Alternatives:** exnref (`try_table`) — newer; also builds and runs (see RESULTS §A25), switch when S5 shows
exnref everywhere in scope.

## S2-4 — `collectgarbage` removed from the sandbox (proposal, pin_script feature `web-parity`)
**Decision:** Recommend removing `collectgarbage` from the sandbox on every target.
**Rationale:** Its only allowed answer, `collectgarbage("count")`, is the Lua heap size, which depends on pointer
size (wasm32 objects are smaller): a script can turn it into commands and make native and web replays differ
(case `collectgarbage_count`). No production table uses it (`grep`), and the repo's own spec already accepts its
absence (`tests/fixtures/spec_sandbox.lua:57` "collectgarbage is either gone or still answers count").
**Alternatives:** keep it and document the divergence (breaks the owner's "same hash exactly" for such scripts);
return a constant (keeps the name, gives meaningless data).

## S2-5 — Lua transcendental math through the Rust `libm` crate on every target (proposal, KHZ patch 32)
**Decision:** Route `sin cos tan asin acos atan2 exp log log2 log10 pow sinh cosh tanh` (and so `^`) through
`khz_*` functions that pin_script implements with `libm =0.2.16`.
**Rationale:** glibc 2.39 vs wasi-libc differ on 0.001 %-17.9 % of 100 000 inputs per function (RESULTS §5.4);
with the routing, 0 of 2 200 000 differ. Matches D9 ("libm for transcendentals"). No production table calls a
math function, so no golden changes (all 11 scenarios PASS natively with it).
**Alternatives:** vendor musl's libm C into lua-src (also identical everywhere, but a second libm in the tree);
accept the divergence (violates the owner's parity rule for user tables using math).

## S2-6 — Memory-cap fault point accepted as platform-dependent
**Decision:** Accept that the exact allocation at which the 2 MiB cap fails differs between wasm32 and x86_64,
document it as a residual risk, and keep the classification test (`MemoryLimit` on both).
**Rationale:** Lua object sizes depend on pointer size; mlua's cap counts real bytes. No exact,
pointer-size-neutral accounting exists without per-object-type knowledge in the allocator. A script reaches the
point only by exhausting its heap, after which it is in the tilt state on both builds.
**Alternatives:** wasm64/memory64 for a separate Lua instance (LP64 layout = x86_64 sizes) — browser support
incomplete (S5); lowering the web cap (does not make the point equal).
