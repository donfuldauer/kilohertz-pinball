# S10 local decisions (PLAN §2.3 rule 8; scratch only, not the repo)

## s10-loader — how the shells load the unchanged modules

**Decision.** Run the wasm-bindgen glue files that the V8 runs used, unchanged, inside the shells: the
`--target nodejs` glue (S1, S7, S8) through a 40-line CommonJS loader (`harness/shellenv.js` `loadCjs`:
`new Function('exports','require','module','__filename','__dirname', src)` with a `require('fs')` shim over
the shell's `read(path,'binary')`), and S4's `--target web` ES-module glue through a dynamic `import()` plus
`initSync`. `TextEncoder`/`TextDecoder` are polyfilled (WHATWG UTF-8 incl. replacement) only because neither
shell has them; output bytes are written with `writeFile` (JSC) / `os.file.writeTypedArrayToFile` (SM).

**Rationale.** The brief allows raw instantiation or a `--target no-modules` glue "where needed"; keeping the
very same glue JS means the only difference from the V8 runs is file I/O and clocks, so every wasm import
(console.error, performance.now, the EH tags, the externref table init, the terminated-instance guard)
behaves as in the browser. No pre-bindgen module exists for a `no-modules` rebuild without recompiling.

**Alternatives.** `wasm-bindgen --target no-modules` (needs the pre-bindgen wasm: a rebuild, and a different
glue than the V8 runs); a hand-written raw import object (re-implements glue logic, a source of harness-only
differences); the wasip1 module under a JS WASI shim (S1's wasip1 route is not the browser route).

## s10-shells — which shell builds stand for Safari and Firefox

**Decision.** JavaScriptCore = WebKitGTK `jsc` bundle of trunk 327409@main (the only Linux JSC builds that
exist: webkitgtk.org keeps ~1 month of trunk builds, x86_64 only); SpiderMonkey = jsshell of Firefox
160.0.1 (newest release) and 153.7.0esr (newest ESR), x86_64 and aarch64 (aarch64 under qemu-aarch64);
the 140.x ESR line is not run.

**Rationale.** Brief requirement 1; the archive listing picks the versions, not a guess. JSC trunk is newer
than any shipping Safari, so a regression-free trunk is the best available proxy; the relation to shipping
Safari builds is reported as "needs S5/web".

**Alternatives.** jsvu (would download the same JSC zip and SpiderMonkey nightly through npm-installed code:
more untrusted code, same bytes); Firefox Nightly jsshell (not a shipping engine).

## s10-tiers — tier flag sets

**Decision.** JSC: default; `ipint` = `--useBBQJIT=0 --useOMGJIT=0`; `bbq` = `--useWasmIPInt=0 --useOMGJIT=0`;
`omg` = `--useWasmIPInt=0 --thresholdForOMGOptimizeAfterWarmUp=0 --thresholdForOMGOptimizeSoon=0`;
`nojit` = `--useJIT=0`. SpiderMonkey: default (baseline+ion lazy tiering), `--wasm-compiler=baseline`,
`--wasm-compiler=optimizing`. Each JSC set was checked by counting BBQ/OMG compilations
(`--dumpBBQDisassembly/--dumpOMGDisassembly`); SpiderMonkey's by `wasmCompileMode()`.

**Rationale.** The brief's flag names (`--useWasmLLInt`, `--useBBQJIT`, `--useOMGJIT`) were checked against
`jsc --options`: the Wasm LLInt no longer exists (IPInt replaced it), and an OMG-only mode is not
expressible (`useWasmIPInt=0` + `useBBQJIT=0` is refused as incoherent), so `omg` is "BBQ first call, OMG
immediately after" — the most OMG the engine allows.

**Alternatives.** `--useWasmLLInt` (absent); OMG via IPInt with thresholds 0 (measured: no OMG compiles at all).

## s10-scope — job plan for the slow tiers

**Decision.** Fast tiers (V8, JSC default/bbq/omg, every x86 SpiderMonkey tier) run everything; JSC `ipint`
runs everything except the uncapped adversarial ticks and runs the 600 000-tick soak for the S1 module only;
JSC `nojit` (the same interpreter with the JS JITs off) runs a subset; aarch64 SpiderMonkey (qemu) runs the
S1 module's 11 scenarios in the default tier and subsets elsewhere (`run_engines.sh` `jobs()` comment).

**Rationale.** The brief allows the soak "only in the optimising tier" when an interpreter tier is too slow;
the interpreter runs at ~430 µs per tick (60x V8) and qemu at ~10-20x slower than x86, and the first plan
(1 668 jobs with every tier everywhere) projected to > 3 h of the shared 4-core machine.

**Alternatives.** Full matrix everywhere (time); dropping interpreter tiers (would miss tier-specific bugs).

## s10-stack-knobs — how stack budgets are varied

**Decision.** V8: `node --stack-size=<KB>`; JSC: `--maxPerThreadStackUsage=<bytes>` (thread stays 8 MiB;
checked equivalent to a real thread of that size with `ulimit -s`); SpiderMonkey: `ulimit -s <KB>` (the
shell has no stack flag and a fixed quota, so below the quota the measure is the real stack use and the
failure is a SIGSEGV of the shell).

**Rationale.** The brief asks for the real knob names; these are the only ones the shells have
(`js --help`, binary strings, `jsc --options`).

**Alternatives.** `--softReservedZoneSize`/`--reservedZoneSize` (they move the error point inside the same
budget, not the budget).

## s10-eh-rebuild — rebuilding the Lua module for the EH-encoding question

**Decision.** Rebuild S8's v2 Lua module twice from a copy of S8's tree (`repo_exnref`), legacy EH and
exnref, into `target/web_s10`, and run both on every engine, instead of reasoning from the S5 table only.

**Rationale.** The brief requires a recommendation "on this evidence"; no exnref build of the v2 module
existed (S2's was deleted). The legacy rebuild's code section equals S8's v2 instruction for instruction
(only data segments with embedded source paths differ), so the two variants differ in EH encoding only.

**Alternatives.** Recommend from the S5 support table alone (no evidence of real behaviour in JSC/SM).

## s10-nan-info — NaN bit differences are information, not failures

**Decision.** Operation-level NaN sign/payload differences between engines (micro-test "NaN-bits-only",
probes `nan_bits`/`fp_ties`) are reported as INFO; every non-NaN result and every NaN's spec class must
match (FAIL otherwise).

**Rationale.** The Wasm spec leaves NaN sign/payload nondeterministic (S7 §7 quote); the sim's hashed state
never held a NaN (all parity runs), and S1 R-4 (canonical NaN in `Fnv64::write_f32`) removes the only way a
NaN's bits could reach a hash. Treating them as failures would flag conforming engines.

**Alternatives.** FAIL on any bit difference (would flag V8 itself against x86 native for min/max).

## s10-known-diffs — rooted differences in the summary

**Decision.** `harness/summarize.py` reads `harness/known_diffs.txt` (pattern + root cause + evidence);
a FAIL matching a rooted entry is listed with its root cause and does not make the exit status non-zero;
any other FAIL does.

**Rationale.** The brief asks to record every difference with a root cause; a known, explained engine
difference (e.g. trap message texts in the JIT rules design) must stay visible, but it is not an unexplained
parity failure.

**Alternatives.** Loosen the comparison itself (hides the difference); leave the run red (no signal for new,
unexplained differences).

## s10-ccalls — Lua C-call limit on small JS stacks (KHZ 38 proposal)

**Decision.** Recommend lowering `LUAI_MAXCCALLS` from 200 to 100 on every target (KHZ 38, same batch as KHZ
33-37) **and** shipping the startup self-test's Lua C-stack check, which refuses user bundles on an engine
whose stack cannot hold even that.

**Rationale.** Measured: the deepest legal Lua C recursion (200 nested `pcall`) needs 1 312 KiB of JSC stack
in BBQ code (176 KiB in V8, 224 KiB in SpiderMonkey), and with a real 1 MiB thread JSC traps
("Module terminated") where native Lua raises "C stack overflow"; with 100 the worst JSC tier needs 728 KiB
and a 1 MiB / 1.25 MiB thread passes. The three production tables need the empty-stack floor (176 KiB in JSC
BBQ), so no production replay changes (measured: 7 replays x 5 engines identical per step and per tick).
Safari's real per-thread budget on iOS is not measurable here; the self-test makes the outcome safe either way.

**Alternatives.** Keep 200 and rely on the self-test only (user bundles refused on any engine with < ~1.3 MiB
of stack: likely all iPhones if WebContent's main thread is 1 MiB); lower further (64: ~0.53 MiB, more user
scripts affected); catch the engine's stack overflow in Lua (measured: no engine lets Wasm `catch_all`
catch it, and unwinding C mid-allocation would not be safe anyway).
