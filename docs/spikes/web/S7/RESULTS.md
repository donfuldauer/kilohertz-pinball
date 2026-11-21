# S7 — web-spike-arm-parity (RESULTS)

Base: `main` at `995935f` (the real tree, read-only). Scratch root `S7` =
`${WEB_SPIKES}/S7`,
`S1` = the sibling `.../web-spikes/S1` (read-only inputs). `S7/repo` = copy of `S1/repo` (S1 patches
0001–0004: sim gating + the `s1_harness`) plus S7 harness probes; flavours are switched in place
by `S7/scripts/flavour.sh base|portable|scalar` so one target dir rebuilds only changed crates.
Every cargo command: `. S7/env.sh` (COMMON env with the S7 overrides `CARGO_BUILD_JOBS=1`,
`CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/web_s7[/mini|/wasm]`, the aarch64 linker and
`qemu-aarch64` runner set by env vars, no repo file edited for the toolchain).

**All aarch64 numbers are qemu-aarch64 8.2.2 user-mode emulation (TCG), not ARM hardware** — but for the
unpatched code they equal the values the repo's own CI printed on an Apple Silicon `macos-latest` runner for
the same commit (fixture `ebfb6f43c9a0b859` and all 5 seam digests, §4.5).

## 0. Bottom line

* **C1 (aarch64 native differs from x86_64 "due to NEON FMA") — CHANGED.** aarch64 native differs
  on every scenario from the first tick a ball spins (fixture tick 6, bot60 86, pat60 28, mb8 8/20),
  by 1 ULP in the ball **orientation quaternion only**: positions, velocities, spins, flippers,
  switches, scores, the script's inputs and the bot's decisions stay bit-equal for all 600 000
  ticks of the soak. The fixture ends at `state_hash=ebfb6f43c9a0b859` on aarch64 (x86_64:
  `25097c21c2c3adc1`; the repo's macOS arm64 CI leg printed the same `ebfb6f43c9a0b859` for `995935f`).
  But the cause is **not** the FMA in `Quat::mul`: NEON `Quat` multiply is
  bit-identical to SSE2 (0 of 1 000 000 inputs differ — its `vfmaq_f32` multiplies by ±1.0, which
  is exact). The causes are (a) the 4-lane dot `vaddvq_f32` = `faddp`,`faddp` = `(x+y)+(z+w)`
  versus SSE2 `(x+z)+(y+w)`, (b) NEON `Vec4::normalize` multiplies by `1/len` where SSE2 divides by
  `len`, and (c) the fused `vfmsq_f32` cross product inside NEON `Quat * Vec3`. Contraction flags
  and glam's `libm` feature change nothing; `scalar-math` gives x86's *scalar* results, not SSE2's.
* **C2 (wasm on ARM equals x86_64) — CONFIRMED (measured, emulated).** The S1 `+simd128` module on
  **V8's ARM64 backend** (official Node v22.23.5 linux-arm64, SHA-256 verified, under qemu-aarch64)
  matches native x86_64 on every tick, `state_hash` and `ext_hash`, for all 11 S1 scenarios
  (3 tables × bot60/pat60/mb8, the CI fixture and the 600 000-tick soak) under default tiering,
  `--liftoff-only` and `--no-liftoff` (TurboFan only): 33/33 PASS. The default (no-simd128) module
  on ARM64 V8 equals the same module on x86_64 V8 on every tick (11/11). A trace of the ARM64 code
  V8 generated (qemu `-d in_asm`) shows `fmul/fadd/fsub/fdiv/fsqrt v.4s` and no fused
  multiply-add. The Wasm spec forbids fusion for these ops (core spec §Numerics, SIMD.md).
* **C3 (a minimal aarch64-only change gives parity without moving x86_64) — CONFIRMED, as a
  target-independent change.** `patches/0006-pin_physics-portable-quat-ops.patch` (+1 module of
  plain `f32` code reproducing glam's SSE2 `Quat` arithmetic, routed through at the 11 `Quat`
  operation sites of `pin_physics`; +173/−13 lines, of which the new module is 154 lines incl. 65 lines
  of tests) makes **aarch64 == x86_64 on every tick of all 11 scenarios** (fixture `25097c21c2c3adc1`) and leaves **x86_64 bit-identical**
  (fixture `25097c21c2c3adc1` via `pin_sim --ticks 10000`, all 11 S1 reference hash files
  reproduced per tick, the 5 `ball_seam_bitident` EXPECTED digests unchanged and the test passing,
  `cargo test -p pin_physics` 833 passed / 0 failed). It needs no `cfg`: the replica equals glam SSE2
  for every input (0 mismatches in 1 000 000 per op on x86_64), so the same code runs everywhere.
  glam `scalar-math` on aarch64 does **not** work (equals x86 scalar-math, 78 % of
  `integrate_orientation` results differ from SSE2). Bonus: with the patch the *default* (no `simd128`)
  wasm build also equals x86_64 on every tick (11/11 on x64 V8 and on ARM64 V8), so parity no longer
  depends on Wasm SIMD.
* **Blast radius: very-high (D34)** for the follow-up — it changes simulation output for every table
  on a supported platform (macOS arm64: every replay/state hash with a spinning ball), and moves no
  value on x86_64 or in the `+simd128` web build. **CI: gate it** (§6): a qemu-aarch64 job on the
  Linux PR runner (fixture + seam digests against the committed x86_64 goldens; ≈ 16 s of emulated
  run time, the cost is the cross build: ≈ 2–4 min per PR warm, 8–12 min cold) and the existing `macos-latest` replay-hash leg asserting
  the same golden on main; amend D9 accordingly.

## 1. Owned claims — verdicts

| claim | verdict | evidence |
|---|---|---|
| C1: aarch64 native differs from x86_64 due to NEON FMA | **changed** (differs: yes; cause: dot-sum order, reciprocal normalise, fused cross — not the FMA in `Quat::mul`) | §3 table (11/11 FAIL, orientation only), §4.1 op table (`quat_mul` same, dot/normalize/mul_vec3 differ), §4.2 disassembly, §4.3 bisection |
| C2: wasm on ARM equals x86_64 | **confirmed** (V8 ARM64 backend, emulated; 33/33 PASS across three tier settings) | §7 |
| C3: minimal aarch64 change gives parity without moving x86_64 | **confirmed** (0006; target-independent replica; x86_64 unchanged by every measure) | §5 |

## 2. Item 1 — toolchain

Installed only via apt (`logs/apt_install.log`, `logs/pkg_versions.txt`): `qemu-user 1:8.2.2+ds-0ubuntu1.18`
(`qemu-aarch64 version 8.2.2`), `gcc-aarch64-linux-gnu 4:13.2.0-7ubuntu1` (gcc 13.3.0, sysroot
`/usr/aarch64-linux-gnu`), `binutils-aarch64-linux-gnu 2.42-4ubuntu2.10` (objdump), `libc6-arm64-cross` /
`libc6-dev-arm64-cross 2.39-0ubuntu8cross1` (pulled in: `libstdc++6-arm64-cross`, `libgcc-s1-arm64-cross`
`14.2.0-4ubuntu2~24.04.1cross1`). `rustup target add aarch64-unknown-linux-gnu` (rustc 1.97.0, LLVM 22.1.6).
No system apt-arch change, no repo file edited: linker, C compiler and runner come from `S7/env.sh`
(`CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc`,
`CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER="qemu-aarch64 -L /usr/aarch64-linux-gnu"`,
`CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc` for the vendored Lua and zstd).
No `-C target-cpu=native`, no added target feature except where an experiment says so.

`rustc --print cfg --target aarch64-unknown-linux-gnu` (`logs/cfg_aarch64.txt`): `target_feature="neon"`
only (default CPU `generic`). There is no `fma` target feature on AArch64 (`rustc --print target-features`
lists `neon`, `fp-armv8`, `fcma`, `fp16`, … but no `fma`): fused multiply-add (`FMADD`, `FMLA`, `FMLS`) is
base ARMv8-A, so it is always available as an instruction and is emitted for explicit fused intrinsics
(`vfmaq_f32` → `fmla`, §4.2). rustc never contracts `a*b+c` on its own: the scalar `w.length()` at the top
of `integrate_orientation` compiles to `fmul,fmul,fadd,fmul,fadd` (no `fmadd`) in the same binary.
macOS: `rustc --print cfg --target aarch64-apple-darwin` (`logs/cfg_aarch64_apple_darwin.txt`) adds
`aes, crc, dotprod, fp16, rcpc, …` (default CPU `apple-m1`); glam picks its NEON module by
`target_arch = "aarch64"` (`glam-0.33.12/src/f32.rs:24-28,53-57`), so macOS runs the same source; without
contraction the extra features cannot change IEEE results (DECISIONS_LOCAL `s7-qemu`).

Speed of qemu-user here (this 4-core VM, shared with two other spikes): native `s1 play` under qemu-aarch64
runs the fixture at **11 800 ticks/s** (0.85 s for 10 000 ticks), 1-ball 60 s games at 6 400–12 400 ticks/s,
9-ball stress at 740–3 300 ticks/s, the 600 000-tick soak in 81 s (`logs/native_aarch64_base.txt`) — about
15–25× slower than native x86_64. So every scenario ran at full length, soak included (no prefixes).

## 3. Item 2 — aarch64 native, measured (`scripts/native_arm.sh aarch64_base`)

`s1 play <bundle> <scn> <scr> <out>` (the S1 harness: recorded script stream, every tick hashed; same
inputs `S1/rec/*`, `S1/bundles/*`) cross-built with `cargo build --release --target aarch64-unknown-linux-gnu
-p s1_harness --features native --bins` on the unpatched sim, run under qemu, compared per tick with
`S1/native/<scn>.hashes` by `S1/harness/compare.py` (`logs/native_aarch64_base.txt`):

| scenario | ticks | result | first diverging tick (state_hash and ext_hash) | x86_64 / aarch64 hash at that tick | aarch64 final state_hash | field that differs (`s1 dump` both sides) |
|---|---|---|---|---|---|---|
| replay_table_fixture10k | 10 000 | FAIL | 6 | 44599810bd85b72b / 69cb3b4fc6103c64 | **ebfb6f43c9a0b859** (x86: 25097c21c2c3adc1) | ball `rot` w: `3f7fff8a` vs `3f7fff8b` |
| apex_circuit_bot60 | 60 000 | FAIL | 86 | c8ae0e983a841b3d / 672a7c753c5f335a | 92cb29a1e583883c | `rot` w `3f7ffde2` vs `3f7ffde3` |
| apex_circuit_pat60 | 60 000 | FAIL | 28 | 1f653090d653c715 / b7cdcb25606d169e | 9a17587cc383b2e2 | `rot` only |
| apex_circuit_mb8 | 20 000 | FAIL | 8 | 8edeacc2ad425771 / c51130a3136517ff | 1f93093a71a9a95a | `rot` z,w `3be06f19 3f7ffe6c` vs `3be06f1a 3f7ffe6d` |
| sky_deck_bot60 | 60 000 | FAIL | 86 | 1ad622a7b7deb03d / aaad0ad1240e8b5a | 25a7458c371b7ddf | `rot` only |
| sky_deck_pat60 | 60 000 | FAIL | 28 | dbe2a234eea46f15 / 09f5df34ddc7ab9e | ce9eb11d97fbef81 (= x86: ball drained) | `rot` only |
| sky_deck_mb8 | 20 000 | FAIL | 20 | 78769dbc47c000bc / 57e429c169449b74 | a4d8b46e67263bc5 | `rot` of 2 balls, 1 ULP |
| feature_lab_bot60 | 60 000 | FAIL | 86 | 3cc54f3c715d7be7 / 0c1189be7868f738 | 503d35a2a81336dc | `rot` only |
| feature_lab_pat60 | 60 000 | FAIL | 28 | 76a4ebd494ad9be0 / a17c7407ba86b247 | 865f852fce618b7e (= x86) | `rot` only |
| feature_lab_mb8 | 20 000 | FAIL | 8 | 6d407ffdac97e650 / 351a22b332f1acfe | 7ce1a1fc69ef06df | `rot` only |
| apex_circuit_bot600 (soak) | 600 000 | FAIL | 86 | c8ae0e983a841b3d / 672a7c753c5f335a | 21c1f16f72987728 | `rot` only (also at 300 000) |

* **Orientation-only.** `s1 dump` (`S7/diverge/*`, compared with `S1/diverge/*_native_*` and fresh x86 dumps) at
  the first diverging tick and at 30 000, 60 000 and 300 000: the *only* differing line is the ball `rot`
  quaternion; `pos`, `vel`, `ang` (spin), flippers, switches and counters are bit-equal. Scores are equal in every
  scenario (e.g. apex bot60 51 480, soak 46 030). The playback reports `script_digest_mismatch=none` everywhere: the
  Lua script saw identical switch edges and `HostView` for the whole run. When the last ball drains the state
  hash reconverges (sky_deck_pat60, feature_lab_pat60 final hashes equal x86's).
* **Real Lua on aarch64 is identical.** `scripts/lua_record_arm.sh aarch64_base` re-recorded all 10 scenarios on
  aarch64 with the real Lua rules (vendored C Lua compiled by `aarch64-linux-gnu-gcc`):
  33/33 PASS (`logs/lua_record_aarch64_base.txt`) — every `.scn`, `.scr` (the script's per-step RNG draws, input
  digest and command queue) and bot `.khzrec` is **byte-identical** to S1's x86_64 recording, and the real-Lua
  per-tick hashes equal the aarch64 playback hashes. So the aarch64 difference is in the Rust sim only.
* **A third distinct result.** aarch64 is neither x86 SSE2 nor scalar glam: it equals wasm-default/scalar for the
  first ticks (tick 6 hash `69cb3b4fc6103c64` is the scalar wasm value) and leaves it later (fixture tick 58,
  apex bot60 tick 408, sky mb8 tick 22 vs `S1/runs/base/bindgen`).
* **Probes** (`probes/aarch64_base_*.txt` vs `probes/x86_64_base_*.txt`): S1's `glam_ops` on aarch64 differs from
  the SSE2 column `S1/a13/glam_ops.txt` in 13 of 15 rows (only `vec3_ops`, `libm_trig` equal;
  `probes/glam_ops_x86_vs_aarch64.txt`). That probe builds its inputs with glam's own `normalize()`, so every
  quaternion row inherits the `normalize` difference; the isolating probe `glam_iso` (inputs from plain scalar
  code, §4.1) shows which ops really differ. `fp_ties` / `nan_bits`: generated NaNs are `7fc00000` on aarch64 and
  `ffc00000` on x86_64 (`0/0`, `inf-inf`, `sqrt(-1)`, `libm::sqrtf(-1)`); `f32::max(+0,-0)` and `max(-0,+0)` give
  `+0` on aarch64 and `-0` on x86_64, `(-0f32).min(0.0)` gives `-0` vs `+0` (`fminnm`/`fmaxnm` vs `minss`/`maxss`
  lowering). No run reached these (every hash matches after the fix, §5), but they are the residual
  cross-ISA differences outside glam (§10).

## 4. Item 3 — the cause, pinned

### 4.1 Operation level (inputs bit-identical on every target)

`mini/` (self-contained crate: glam 0.33.12 + libm only, 1 000 000 PCG inputs incl. ±0, built for each variant in
seconds; `probes/mini/TABLE.txt`) and the harness probe `s1 probe glam_iso` (200 000 inputs, through the sim's own
`integrate_orientation`; `probes/glam_iso_x86_vs_aarch64_base.txt`) agree:

| op (glam 0.33.12) | aarch64 NEON vs x86_64 SSE2 | NEON results ≠ SSE2 (of 1 000 000) | why |
|---|---|---|---|
| `Quat * Quat` (`mul_quat`) | **same** | 0 | NEON `vfmaq_f32(a, b, CONTROL)` multiplies by ±1.0 (exact) — the FMA cannot round differently |
| `Quat::dot`, `length_squared`, `Vec4::dot`, `Vec4::length` | differ | 267 992 (`length_squared`) | `vaddvq_f32` (`neon.rs:62-65`) = `(x+y)+(z+w)`; SSE2 `dot4_into_m128` = `(x+z)+(y+w)` (`sse2.rs:75-80`) |
| `Quat::normalize`, `Vec4::normalize` | differ | 646 011 | the dot above **and** `self * length_recip()` (`f32/neon/vec4.rs:644-649`) vs `self / sqrt(dot)` (`f32/sse2/vec4.rs:654-662`) |
| `Quat * Vec3` (`mul_vec3a`), `conjugate * Vec3` | differ | 287 323 / 287 588 | `Vec3A::cross` = `vfmsq_f32` (fused, `f32/neon/vec3a.rs:293`) inside `f32/neon/quat.rs:967-981`; SSE2 cross is `sub(mul, mul)` without `fma` (`sse2.rs:144-154`, `f32/sse2/vec3a.rs:279-291`) |
| `Quat::from_rotation_arc` | differ | 623 018 | its final `normalize()` |
| `integrate_orientation` (the sim, per tick per ball) | differ | 323 538 | `(dq * q).normalize()`: the normalize |
| `Vec3A::cross` | differ | — | `vfmsq_f32` (fused) |
| `Vec3A::dot`, `Vec3A::length` | same | — | 3-lane `vaddvq` of `[x,y,z,0]` = `(x+y)+z`, same as SSE2 `dot3_in_x` |
| `Vec4::min/max` | differ | — | ±0/NaN tie rule (`fmin` vs `minps`) |
| `Mat4 * Mat4`, `transform_point3` | same | — | unfused mul/add in both |
| `Mat4::inverse` | differ | — | NEON result equals glam's scalar result (same digest as `scalar-math`) |
| `Quat::lerp/slerp` | differ | — | dot + `vfmaq_f32` (`f32/neon/quat.rs:790`) |
| `Vec3` (scalar type) ops, `Vec3::min/max`, `Vec3A::lerp`, `libm` sin/cos/atan2/exp/sqrt/acos/asin/pow/hypot, `f32::mul_add` | same | — | scalar code / exactly rounded |
| core `f32::min/max` with ±0 ties | differ | — | not glam: `fminnm/fmaxnm` vs `minss/maxss` (§3) |

The sim uses only `Quat` mul, `* Vec3`, `conjugate`, `normalize`, `length_squared`, `from_rotation_arc` (12 calls at 11
sites, all in `pin_physics`: `ball/contact.rs:377` per tick, `flipper/mod.rs:199-200`, `world/shapes.rs:97-99,188,213-214`,
`world/static_world.rs:143,150`) and `Mat4::transform_point3` at load (`pin_sim/src/load/mod.rs:401,736`, same on
both). No `Vec3A`, `Vec4`, `Affine3A`, `lerp`/`slerp` or glam trig is used by any sim crate (grep, §4.4).

### 4.2 Instruction level (`aarch64-linux-gnu-objdump -d -C --disassemble='pin_physics::ball::contact::integrate_orientation'`)

The function is a real symbol in both release binaries (`disasm/aarch64_base_integrate_orientation.txt`,
`disasm/x86_64_base_integrate_orientation.txt`). After the two `libm` calls, aarch64 (unpatched):

```
b096fc: fmov  v4.4s, #1.0            ; CONTROL_WZYX built as [1,-1,1,-1] (fneg v2.2d below)
b0970c: fmul  v0.4s, v1.4s, v0.s[0]  ; lxrw_lyrw_lzrw_lwrw = w * rhs
b09718: fmul  v3.4s, v3.4s, v6.s[0]
b09720: fmul  v1.4s, v2.4s, v1.s[0]
b09724: fneg  v2.2d, v4.2d
b09730: fmul  v4.4s, v5.4s, v4.s[0]
b09734: fmul  v3.4s, v3.4s, v6.4s    ; * CONTROL_ZWXY
b0973c: fmla  v0.4s, v2.4s, v4.4s    ; vfmaq_f32 (quat.rs:916): fused, but times +-1.0 -> exact
b09740: fmla  v3.4s, v5.4s, v1.4s    ; vfmaq_f32 (quat.rs:918): same
b09748: fadd  v0.4s, v0.4s, v3.4s    ; result0 + result1
b0974c: fmul  v1.4s, v0.4s, v0.4s    ; normalize: squares
b09750: faddp v1.4s, v1.4s, v1.4s    ; vaddvq_f32: [x+y, z+w, ...]
b09754: faddp s1, v1.2s              ;             (x+y)+(z+w)   <- differs from SSE2
b09758: fsqrt s1, s1
b0975c: fdiv  s1, s2, s1             ; 1/len (length_recip)       <- differs from SSE2
b09760: fmul  v0.4s, v0.4s, v1.s[0]  ; q * (1/len)
```

x86_64 (same source, SSE2 path, `objdump -d -C`):

```
1203f5b: mulps  CONTROL,%xmm3 ; 1203f62: mulps CONTROL,%xmm2 ; 1203f6f: mulps CONTROL,%xmm1
1203f6c: addps  %xmm0,%xmm2   ; 1203f76: addps %xmm3,%xmm1   ; 1203f79: addps %xmm2,%xmm1   (unfused mul, add)
1203f7f: mulps  %xmm1,%xmm0             ; squares
1203f85: shufps $0x4e,%xmm0,%xmm2       ; [z,w,x,y]
1203f89: addps  %xmm0,%xmm2             ; [x+z, y+w, ...]
1203f8f: shufps $0xb1,%xmm2,%xmm0
1203f93: addps  %xmm2,%xmm0             ; (x+z)+(y+w)
1203f96: sqrtps %xmm0,%xmm0
1203f99: divps  %xmm0,%xmm1             ; q / len
```

`Quat * Vec3` on aarch64 (`<pin_physics::world::shapes::Primitive>::aabb`, `::closest`): `fmla v.4s` (the
`vfmsq_f32` cross, with the subtrahend negated by LLVM) and `faddp s, v.2s` sums
(`disasm/aarch64_base_Primitive_aabb.txt`).

### 4.3 Bisection by experiment (`mini`, 1 000 000 inputs; `probes/mini/TABLE.txt`)

| experiment | build | result |
|---|---|---|
| baseline | `cargo build --release --target aarch64-unknown-linux-gnu` | 15 of 25 op digests ≠ x86_64 (14 glam ops + core `f32::min/max`; table above); the 7 replica digests are not part of the 25 |
| (i) glam `scalar-math` on aarch64 | `--features scalar-math` (→ `glam/scalar-math`) | **every digest = x86_64 `scalar-math`**, not SSE2: `Quat` mul now differs (740 854 of 1e6), `integrate_orientation` 784 499 of 1e6 differ from SSE2; `Quat * Vec3` and `Vec4::min/max` become equal. Not a fix. Sim-level confirmation in §5.5. |
| (ii) glam `libm` feature | `--features glam-libm` | **identical to baseline** (it only swaps glam's trig, which the sim does not call) |
| (iii) local replacement of `Quat` mul/dot/normalize/`*Vec3`/`from_rotation_arc` with explicit unfused code in SSE2 order (`quat_ops.rs`) | same build | **all 7 replica digests = x86_64 SSE2** on aarch64 and on x86_64; on x86_64 the replica equals glam SSE2 for **every** input (0 mismatches in 1 000 000 per op) |
| (iv-a) `RUSTFLAGS="-C llvm-args=-fp-contract=off"` | aarch64 | **identical to baseline**: glam calls `vfmaq_f32`/`vfmsq_f32` intrinsics (`llvm.fma`), which fuse regardless of contraction flags, and the dot/normalize differences are not contraction at all |
| (iv-b) `RUSTFLAGS="-C target-feature=-neon"` | aarch64 | unusable: rustc warns "target feature `neon` must be enabled to ensure that the ABI of the current target can be implemented correctly … will become a hard error" (issue #116344; `logs/mini_minus_neon.log`), glam still compiles its NEON module (it keys on `target_arch`), and the binary dies with `qemu: uncaught target signal 11 (Segmentation fault)` |

### 4.4 What else could differ (checked)

`libm` 0.2.16 on aarch64 uses hardware `fmadd`/`frintn`/`fsqrt` only for `fma`/`rint`/`sqrt`
(`libm-0.2.16/src/math/arch/mod.rs:22-35`), all exactly rounded: `libm` digests equal (§4.1). glam's own trig
(`Quat::from_axis_angle`, `from_rotation_x/y/z`) uses **std** `sin_cos` → the platform libm (glam built without its
`libm` feature, `glam-0.33.12/src/f32/math.rs:193-196`); no sim crate calls those (grep of
`from_axis_angle|from_rotation_[xyz]|from_euler|angle_between|slerp|…` in pin_core/physics/input/sim/assets/script/tablegen:
none; only `from_rotation_arc`'s unreachable 180° branch). glibc sinf/cosf differ from the `libm` crate on 1.5 % of
inputs but identically on x86_64 and aarch64 (`probes/trig_*.txt`); Apple's libm is not measured (§10).
Bundles: the Feature Lab bundle generated on aarch64 (`s1_build_feature_lab` under qemu) is **byte-identical** to
S1's x86_64 one (`diff -r` clean, `logs/feature_lab_build_aarch64.log`); Apex/Sky in §5.6.

### 4.5 Real Apple Silicon confirms the emulated numbers

The repo's own CI already ran the unpatched base on Apple hardware: GitHub Actions run `44761528119` (ci.yml, push of
`995935f`, read with `gh run view … --log`, `ci_logs/SUMMARY.txt`), job `replay-hash (aarch64-apple-darwin)` on
`macos-26-arm64/20261117.0516` (macOS 26.7.2) printed **`recorded: state_hash=ebfb6f43c9a0b859`** and the seam digests
**`0xd79f654b4dd0d3b0, 0xcd0a64cdfdf642ba, 0x23d5ae21c39688ea, 0x67626b0765d4b20a, 0x6f1c95f661c0e1bd`** — bit for bit
the values qemu-aarch64 gives here for the same commit (fixture via `pin_sim --ticks 10000` with real Lua,
`logs/aarch64_base_ci_fixture.txt`; seam digests `logs/test_aarch64_base_ball_seam_bitident.log`). The Windows
x86_64 (MSVC) leg printed `25097c21c2c3adc1` and the 5 x86_64-linux digests. So: qemu reproduces Apple M-series
results for this code, Windows x86_64 already equals Linux x86_64, and aarch64 is the only native outlier.

## 5. Item 4 — the smallest change: `patches/0006-pin_physics-portable-quat-ops.patch`

### 5.1 The patch

`pin_physics::quat_ops` (new, `src/quat_ops.rs`): `mul`, `length_squared`, `normalize`, `mul_vec3`,
`from_rotation_arc` as plain `f32` expressions that reproduce glam 0.33.12's SSE2 code lane by lane — same
products, same grouping, a division (not a reciprocal multiply), no fused op:

```rust
pub fn mul(a: Quat, b: Quat) -> Quat {          // sse2/quat.rs:885-919 (rtm::quat_mul order)
    Quat::from_xyzw(
        (a.x * b.w + a.w * b.x) + (a.y * b.z - a.z * b.y),
        (a.w * b.y - a.x * b.z) + (a.z * b.x + a.y * b.w),
        (a.x * b.y + a.w * b.z) + (a.z * b.w - a.y * b.x),
        (a.w * b.w - a.x * b.x) + (-(a.z * b.z) - a.y * b.y),
    )
}
pub fn length_squared(q: Quat) -> f32 { (q.x * q.x + q.z * q.z) + (q.y * q.y + q.w * q.w) } // sse2.rs:75-80
pub fn normalize(q: Quat) -> Quat { let len = length_squared(q).sqrt(); /* q.x / len, … */ } // sse2/vec4.rs:654-662
pub fn mul_vec3(q: Quat, v: Vec3) -> Vec3 { /* (v*(w²-b²) + b*(2 v·b)) + (b×v)*(2w), dot (x+y)+z, cross unfused */ }
pub fn from_rotation_arc(from: Vec3, to: Vec3) -> Quat { /* glam's code with quat_ops::normalize */ }
```

Call sites changed (no `cfg`, every target): `ball/contact.rs:377` (`integrate_orientation`, per tick per ball),
`flipper/mod.rs:199-200` (`plane_axes`), `world/shapes.rs:97-99,188,213-214` (cuboid AABB and closest point),
`world/static_world.rs:143,150` (cuboid rotation check and normalisation). Unit test + proptest
(`quat_ops::tests::against_glam_sse2::matches_glam_sse2_bit_for_bit`, compiled where glam has its SSE2/simd128 path)
assert bit equality with glam. `git -C /home/user/kilohertz-pinball apply --check` clean against `995935f` (no S1
patch needed; the harness patches 0001–0004 are only for the spike tooling). `cargo fmt --check -p pin_physics`
clean; `cargo clippy -p pin_physics --all-targets -- -D warnings` rc=0 on x86_64 and on
`--target aarch64-unknown-linux-gnu`; `--target wasm32-unknown-unknown` (lib) rc=0 (`--all-targets` on wasm32 fails
before and after on the dev-dependency `getrandom` without `wasm_js`, unrelated). No allocation, no branch on a
named behaviour (rules 4, 6). Rationale for "no cfg": DECISIONS_LOCAL `s7-route`.

### 5.2 aarch64 == x86_64 with the patch (qemu)

`scripts/flavour.sh portable` + `cargo build --release --target aarch64-unknown-linux-gnu -p s1_harness --features
native --bins` + `scripts/native_arm.sh aarch64_portable` (`logs/native_aarch64_portable.txt`): **11/11 PASS** —
every tick's `state_hash` and `ext_hash` equal `S1/native/<scn>.hashes`:

| scenario | ticks | aarch64 + 0006 vs x86_64 | final state_hash |
|---|---|---|---|
| replay_table_fixture10k | 10 000 | PASS every tick | **25097c21c2c3adc1** |
| apex_circuit_bot60 / pat60 / mb8 | 60 000 / 60 000 / 20 000 | PASS / PASS / PASS | 2b7b1eec564b8cdc / b9c2a55ba04954e9 / 2c52fb33d519ffb7 |
| sky_deck_bot60 / pat60 / mb8 | 60 000 / 60 000 / 20 000 | PASS / PASS / PASS | 1f0509f5802dc2e5 / ce9eb11d97fbef81 / 777557dae8e5531b |
| feature_lab_bot60 / pat60 / mb8 | 60 000 / 60 000 / 20 000 | PASS / PASS / PASS | 2040cea9ef77a2e0 / 865f852fce618b7e / 0c64556eecf5c210 |
| apex_circuit_bot600 (soak) | 600 000 | PASS every tick | a4b944ad641338d1 |

Also: the CI fixture through the real CLI and real Lua on aarch64 — `pin_sim <fixture> --ticks 10000
--record-inputs r.khzrec` then 10 × `--replay-inputs` under qemu — prints `state_hash=25097c21c2c3adc1`, 10/10
replays equal (11.7 s; `logs/aarch64_portable_ci_fixture.txt`; unpatched: `ebfb6f43c9a0b859`, the macOS CI value).
`s1 dump` at ticks 6, 10 000, 86, 30 000, 28, 8, 300 000: aarch64+0006 output identical to x86_64 (`diverge/*_aarch64_portable_*`).

Op level (inputs identical on all targets; `probes/*_portable_*`): `s1 probe quat_ops_iso` gives the same 6 digests on
aarch64 and x86_64, each equal to x86_64 glam SSE2's `glam_iso` digest of the same op (`quat_mul 08ed4ce9f1f70efd`,
`quat_length_squared 7fc362950bb8a671`, `quat_normalize 6b8240f2fa43d429`, `quat_mul_vec3 aca77dbca4e27a03`,
`quat_conj_mul_vec3 7663b70970a3877e`, `quat_from_rotation_arc 6d2fcb051d12a602`), and `glam_iso`'s
`integrate_orientation` is `3672ed63d1bb4455` on x86_64, x86_64+0006 and aarch64+0006. `mini`: 0 mismatches between the
replica and glam SSE2 in 1 000 000 inputs per op on x86_64, replica digests equal on aarch64. S1's `s1 probe glam_ops`
on aarch64+0006 still differs in 13 of 15 rows, by design: it measures glam itself (unchanged) with inputs made by
glam's `normalize()` (DECISIONS_LOCAL `s7-glam-ops-proof`).

`ball_seam_bitident` on aarch64 (`cargo test --target aarch64-unknown-linux-gnu -p pin_physics --test
ball_seam_bitident --no-run`, binary run under qemu with `--nocapture`):

| digest | x86_64 EXPECTED | aarch64 before (= macOS CI) | aarch64 + 0006 | aarch64 + 0006 + test inputs via `quat_ops` (experiment) |
|---|---|---|---|---|
| spec_scenario | 0xa8e8957e1b4cee40 | 0xd79f654b4dd0d3b0 | **same** | same |
| spec_scenario_hook | 0x9da6a4436316c72e | 0xcd0a64cdfdf642ba | **same** | same |
| box | 0x7850af7058aace12 | 0x23d5ae21c39688ea | **same** | same |
| belt | 0x710c25cd832c76c2 | 0x67626b0765d4b20a | **same** | same |
| slope | 0x1764f34e6e37436f | 0x6f1c95f661c0e1bd | 0x90d5ff5a5ff222dd | **same** |

The test only asserts EXPECTED on x86_64-linux; on aarch64 it passes (run-to-run) and prints the lines above
(`logs/test_aarch64_{base,portable}_ball_seam_bitident.log`, `logs/test_aarch64_portable_seam_exp_inputs.log`).
The remaining `slope` difference is in the **test's** input construction: `slope_scenario` builds its colliders and
ball positions with glam `Quat * Quat` and `rot * v(...)` (NEON). Routing those three expressions through `quat_ops`
(`patches/exp-seam-test-portable-inputs.patch`, experiment only) gives 5/5 on aarch64 and leaves x86_64 at the
EXPECTED values. The test also builds rotations with `Quat::from_rotation_x/y` → std `sin_cos` (platform libm); for
these three angles std and `libm` give identical bits on glibc x86_64 and aarch64 (`probes/trig_*.txt`), Apple's libm
is not measured — the follow-up should build them with `libm::sinf/cosf` (§10).

Full suite on aarch64: `cargo test --target aarch64-unknown-linux-gnu -p pin_physics` (runner qemu) with 0006:
**832 passed, 0 failed, 14 ignored** (the SSE2-only proptest is not compiled there; `logs/test_aarch64_portable_pin_physics.log`,
1 659 s of emulated test time).

### 5.3 x86_64 untouched

With 0006 applied in `S7/repo` (x86_64 native build):

* (a) `pin_sim crates/pin_sim/tests/fixtures/replay_table --ticks 10000` → **`state_hash=25097c21c2c3adc1`**
  (`logs/x86_portable_pin_sim_fixture.txt`).
* (b) `scripts/native_arm.sh x86_64_portable x86`: **all 11 S1 reference files reproduced on every tick** (fixture, all
  9 table scenarios, the 600 000-tick soak; `logs/native_x86_64_portable.txt`); the unpatched build gives the same 11/11
  (`logs/native_x86_64_base.txt`).
* (c) `cargo test -p pin_physics --test ball_seam_bitident -- --nocapture`: 5 × `same`, `test result: ok`
  (`logs/test_x86_portable_ball_seam_bitident.log`); full `cargo test -p pin_physics`: **833 passed, 0 failed,
  14 ignored** (S1's 831 + the 2 new `quat_ops` tests; `logs/test_x86_portable_pin_physics.log`).
* (d) wasm: the `+simd128` build with 0006 (`RUSTFLAGS="-C target-feature=+simd128" cargo build --release --target
  wasm32-unknown-unknown -p s1_harness --lib`, `wasm-bindgen 0.2.130 --target nodejs`) through S1's
  `harness/run_bindgen.mjs`: **11/11 PASS** vs native x86_64 on x86_64 Node (`logs/wasm_portable_simd_x86node.txt`) and
  **11/11 PASS** on ARM64 V8 under qemu (`logs/wasm_portable_simd_armnode.txt`).
* Bonus: the **default wasm build (no `simd128`) with 0006 also matches** native x86_64 on every tick, 11/11 on x86_64
  Node and 11/11 on ARM64 V8 (`logs/wasm_portable_nosimd_{x86node,armnode}.txt`). With 0006, `simd128` is no longer
  needed for parity (it stays a speed/size choice), which removes the parity dependence on Wasm SIMD support in
  browsers.
* Op-level x86_64: the replica equals glam SSE2 for 1 000 000 of 1 000 000 inputs per op (`probes/mini/x86_64_default.txt`).

The patch changes codegen, not results: e.g. on aarch64 LLVM SLP-vectorises the replica into 2-lane ops
(`disasm/aarch64_portable_integrate_orientation.txt`: `fmul/fadd/fsub v.2s`, two-operand `faddp s, v.2s`, **no**
`fmla/fmadd`, versus 2 × `fmla` + 4-lane `faddp` before); per-lane IEEE ops in the same grouping cannot change
results, and every measurement above confirms it.

### 5.4 Cost

Not a hardware measurement on ARM: under qemu, `s1 bench` steady per-tick mean, 3 interleaved runs, median:
apex bot60 107.8 → 117.6 µs (×1.09), sky mb8 1 425 → 1 478 µs (×1.04) (`bench/qemu_bench_summary.txt`; emulation
noise ±10 %). x86_64 native (real hardware, loaded VM): apex bot60 5.32 → 4.69 µs, sky mb8 63.5 → 58.9 µs (×0.88–0.93,
i.e. within noise; `bench/x86_bench_summary.txt`). The replica is ~30 scalar flops per quaternion step; the tick is
dominated by collision.

### 5.5 Rejected candidates, measured at sim level

* glam `scalar-math` on aarch64 only (`patches/exp-glam-scalar-math-aarch64.patch`: a
  `[target.'cfg(target_arch = "aarch64")'.dependencies] glam = { workspace = true, features = ["scalar-math"] }` in
  pin_physics, which switches glam for the whole aarch64 build by feature unification): **10/10 FAIL vs x86_64**
  (same first ticks as today) but **10/10 PASS vs x86_64 + scalar-math** (`S1/runs/scalar/*.native_play.hashes`, fixture
  `77b7fda16bae55d2`; `logs/native_aarch64_scalar*.txt`). Scalar glam is ISA-portable but is not SSE2: using it would mean
  S1's option (a) on every target (moves x86_64 goldens). Rejected.
* `cfg(target_arch = "aarch64")` around the same calls: equal results by construction (the replica is exact on x86_64);
  rejected only because then x86_64 CI would never execute the replica (DECISIONS_LOCAL `s7-route`).
* Patching glam's NEON backend (`[patch.crates-io]`): not needed; would vendor a third-party crate and still leave
  `Vec4::min/max`, `Mat4::inverse`, `lerp/slerp` to port.

### 5.6 Bundles (table hash) on aarch64

The release workflow builds each package's bundles on its own runner (`release.yml:320-322`, macOS = arm64), so the
table hash must not depend on the ISA. All three included tables built on aarch64 (unpatched; `tables/<t>/build.sh`
with `CARGO_BUILD_TARGET=aarch64-unknown-linux-gnu` and the qemu runner; Feature Lab via `s1_build_feature_lab`) are
**byte-identical, file for file, to S1's x86_64 bundles** (`diff -rq`, `logs/bundle_*_aarch64_base.log`), and again
with 0006 applied (`bundles_aarch64_portable/`, `logs/bundle_*_aarch64_portable.log`). Recordings therefore load on every
platform (same `table_hash`), before and after the change.

## 6. Item 5 — blast radius and CI gating

**Blast radius: very-high (D34)** for the follow-up implementation of 0006: it changes simulation output for every table
on a supported platform. Evidence of what moves and what does not:

| where | moves? | evidence |
|---|---|---|
| macOS arm64 (aarch64) native | **yes**: every ball orientation once a ball spins, hence every `state_hash`/`ext_hash` from that tick (fixture `ebfb6f43c9a0b859 → 25097c21c2c3adc1`; 5 seam digests → the x86_64 EXPECTED for 4 of 5 at once, `slope` after the test-input change). Old macOS-arm64 replays keep loading (same inputs, same table hash) but replay to the x86_64 values | §3, §5.2, macOS CI §4.5 |
| Linux/Windows x86_64 native | **no** (bit-identical: fixture, 11 per-tick reference files incl. the soak, 5 seam digests, 833 tests) | §5.3, Windows CI §4.5 |
| web `+simd128` (S1's recommendation) | **no** (11/11 on x64 V8, 11/11 on ARM64 V8) | §5.3 (d) |
| web without `simd128` | yes, to the x86_64 values (gains parity: 11/11 on x64 V8 and on ARM64 V8) | §5.3 |
| bundles / table hashes | no (all 3 bundles byte-identical on x86_64, aarch64 and aarch64 + 0006) | §5.6 |
| positions, velocities, switches, scores, script inputs | no on any target (orientation does not feed back in any measured run) | §3 |

**CI recommendation: (2) + (3), and amend D9.** Runner labels in the real tree's workflows: `ubuntu-latest`,
`windows-latest`, `macos-latest` (aarch64-apple-darwin, ci.yml:173/284/381/504, release.yml:158-162); no
`ubuntu-24.04-arm` anywhere.

1. **(3) PR gate: qemu-aarch64 on the Linux runner**, in the `replay-hash` job (or a sibling job): `apt-get install
   qemu-user gcc-aarch64-linux-gnu`, `rustup target add aarch64-unknown-linux-gnu`, `cargo build --release --target
   aarch64-unknown-linux-gnu -p pin_sim --bin pin_sim` with the linker/runner env vars, then the existing
   record + 10 replays under `qemu-aarch64 -L /usr/aarch64-linux-gnu` and **fail unless the recorded value equals the
   x86_64 leg's** (or a committed golden), plus `cargo test --target aarch64-unknown-linux-gnu -p pin_physics --test
   ball_seam_bitident` with `RECORDING_TARGET` widened to aarch64. Cost from this spike: emulated run time ≈ 12 s for
   record + 10 replays (11.7 s measured) and ≈ 4 s for the seam test; the cost is the cross build. On this 4-core VM
   at 1 job the full cold aarch64 build of the harness (incl. wasmtime) took 14 min, an incremental `pin_sim` relink 17 s,
   the seam test build 2 min 11 s. On the CI the x86_64 `pin_sim` build is 25 s with a warm cache and the cold native
   macOS build 6 min 55 s (run 44761528119), so expect **≈ 2–4 min per PR with a warm `rust-cache`, ≈ 8–12 min cold**,
   all on the Linux runner (no macOS minutes).
2. **(2) Hardware confirmation on main:** the existing `replay-hash-cross` `macos-latest` leg (Apple Silicon) stops being
   report-only for the fixture and the seam digests: it asserts equality with the x86_64 golden (`25097c21c2c3adc1` and
   EXPECTED). It already runs on every push to main (D40), so no new runner minutes; it catches anything qemu might miss
   (hardware vs TCG) within one push. Keep Windows report-only or gate it the same way (it already equals; recommended:
   gate, it is free).
3. Not (1) "keep report-only": the owner requires native and web replays to match; without a gate the aarch64 path
   regresses silently (it does today). Not an `ubuntu-24.04-arm` job: no such label in use, and qemu gives the same
   numbers (§4.5) on the runner the PRs already use.

**Proposed D9 amendment text (PLAN §2.4 and the decision-log row; for the follow-up, not edited here):**

> **D9 — Determinism.** *Gate:* (1) the same binary on the same machine replays any recording to identical ball state,
> switch-hit counts and scores; (2) **cross-platform equality on the reference set:** the `replay-hash` fixture
> `state_hash` (10 000 ticks) and the 2.2-S seam digests must equal the committed x86_64-linux goldens on aarch64
> (qemu-aarch64 on every pull request; `macos-latest` Apple Silicon on every push to main) and on wasm32 (web workflow),
> and Windows x86_64 on push to main. *Report only:* other cross-OS hashes (the soak `cross-os` job). *To stay equal
> across platforms:* no fast-math, no FMA contraction and no explicit fused or SIMD-width-dependent float ops in the sim
> crates (quaternion arithmetic through `pin_physics::quat_ops`, not glam's SIMD backends), `libm` for transcendental
> functions (also in tests that build reference inputs), fixed iteration order, one sim-owned seeded PCG, and NaNs
> hashed as one canonical pattern.

Decision-log row: "D9 | Determinism | **Same-build determinism and x86_64 = aarch64 = wasm32 equality of the reference
replay and seam digests are gates**; other cross-OS hashes are reported | f32 SIMD paths differ by ISA unless pinned".

## 7. Item 6 — wasm on ARM, measured

Node: official `node-v22.23.5-linux-arm64.tar.xz` from nodejs.org/dist, in its own empty dir `dl/node_arm64`,
SHA-256 `2babb9eb…fd5e09` checked against `SHASUMS256.txt` from the same site (`sha256sum -c`: OK); only `bin/node`
extracted and run; the cross packages already provide `libstdc++.so.6`/`libgcc_s` under `/usr/aarch64-linux-gnu`, so no
extra sysroot. Under `qemu-aarch64 -L /usr/aarch64-linux-gnu`: `process.arch=arm64`,
`process.config.variables.host_arch=arm64`, `target_arch=arm64`, `process.versions.v8=12.4.254.21-node.57`. V8 ran
under qemu-user without any mmap/JIT/signal problem.

Runs (`scripts/wasm_arm.sh <simd|base> <label> [node flags]`, S1's `harness/run_bindgen.mjs` on S1's own
`runs/{simd,base}/pkg_node`; all 11 scenarios at full length, per tick, `state_hash` and `ext_hash`):

| module | V8 flags (tier) | vs native x86_64 | vs the same module on x86_64 V8 | wall (fixture / apex bot60 / soak) |
|---|---|---|---|---|
| `+simd128` (S1 recommended) | default (Liftoff → TurboFan dynamic tiering) | **11/11 PASS** | — | 3.5 s / 14.9 s / 118 s |
| `+simd128` | `--liftoff-only` | **11/11 PASS** | — | 3.2 s / 18.9 s / 156 s |
| `+simd128` | `--no-liftoff` (TurboFan only) | **11/11 PASS** | — | 14.5 s / 29.7 s / 128 s |
| default (no simd128) | default | 11/11 FAIL (expected, S1) | **11/11 PASS** (every tick = `S1/runs/base/bindgen`) | 3.9 s / 16.5 s / 119 s |

(`logs/wasm_arm_{simd_default,simd_liftoff,simd_turbofan,base_default}.txt`; `--jitless` deliberately not used.)
`s1 probe glam_ops` on ARM64 V8: all 15 digests equal the S1 `wasm32 +simd128` column, incl. `vec4_min_max=b968…`.

**ARM64 code proof.** `qemu-aarch64 -d in_asm` of `node --no-liftoff probe_maps.cjs … glam_ops` (and `--liftoff-only`),
blocks filtered to the anonymous executable mappings from the guest's `/proc/self/maps` (V8's code space),
disassembled with `aarch64-linux-gnu-objdump -b binary -m aarch64` (`scripts/jit_insns.py`, `logs/jit/*`):
TurboFan: 1 752 JIT blocks, 45 776 bytes executed, vector ops `fmul v.4s` ×132, `fadd v.4s` ×73, `fsub v.4s` ×24,
`fsqrt v.4s` ×9, `fdiv v.4s` ×8 — **no `fmla/fmls/fmadd/fmsub/fnmadd/fnmsub`** (`fnmul d` ×2 = −(a·b), one rounding);
Liftoff: 1 892 blocks, 63 420 bytes, same picture, no fused op. This is the code that produced the PASS results.

**Spec (fetched with curl from raw.githubusercontent.com into `dl/wasm_spec/`, `SOURCES.txt`):**
* Core spec `document/core/exec/numerics.rst` (WebAssembly/spec main `d0b2e8d5`), §Floating-Point Operations, lines
  944-947: "Floating-point arithmetic follows the IEEE 754 standard, with the following qualifications: All operators use
  round-to-nearest ties-to-even". §NaN Propagation, 1046-1052: a NaN result's "sign is non-deterministic", payload
  canonical if inputs are canonical, otherwise "picked non-deterministically". `fma` (line 1258) is used only by
  `relaxed_madd`/`relaxed_nmadd` (2355-2367: "Relaxed multiply-add allows for fused or unfused results").
* SIMD proposal `proposals/simd/SIMD.md` (WebAssembly/simd main `a78b98a6`), §Floating-point interpretations 128-131:
  "the rules about NaN propagation and default NaN values are the same, and all operations use the default
  roundTiesToEven rounding mode"; §Floating-point arithmetic 1007-1031: `f32x4.add/sub/div/mul/sqrt` = "Lane-wise IEEE
  addition/subtraction/division/multiplication/squareRoot". So a conforming engine cannot fuse `f32x4.mul` + `f32x4.add`
  (each must round), and core Wasm has no FMA opcode; only relaxed SIMD (`relaxed-simd/Overview.md` 28-34: "FMA,
  reciprocal instructions, sum reduction") may differ, and S1 found 0 relaxed ops in the module (A12).
* Spec-allowed nondeterminism the sim could meet: NaN sign/payload — measured here: `0/0` is `ffc00000` on x64 V8 and
  `7fc00000` on ARM64 V8 (`probes/wasm_*_fp_ties.txt`), so NaN bits differ **between browsers on different CPUs**, not
  only native vs web (S1 R-4 canonical-NaN hashing is needed for web-vs-web too). `f32x4.pmin/pmax` (glam `Vec4::min/max`
  under simd128) and scalar `min/max` ties: deterministic by the spec's definitions; scalar `f32::min/max` ±0 ties in our
  wasm equal x86_64 native on both CPUs. No run reached a NaN or a ±0 tie in hashed state (all PASS).
* Unverified: JavaScriptCore (Safari) and SpiderMonkey (Firefox) backends on ARM, and real ARM hardware for V8 — needs
  S5/web or a device; the spec rules above bind them as well.

## 8. Item 7 — S1 sentences to correct

| S1 RESULTS.md location | sentence (abridged) | S7 verdict | replacement sentence |
|---|---|---|---|
| §0 bullet 1 | "Bit-identical native↔web is achieved, with no change to native and no change to any sim source: build the web module with `-C target-feature=+simd128`…" | **changed** (true for x86_64 native only) | "Bit-identical x86_64-native↔web is achieved with `+simd128` and no sim change. aarch64 native (macOS) differs from both (§3 of S7); with S7's `pin_physics::quat_ops` change (no x86_64 output change) x86_64, aarch64 and wasm (with or without `simd128`) are bit-identical." |
| §0 bullet 2 | "Root cause: glam 0.33.12 picks its SSE2 code on x86_64 and its scalar code on wasm32 without simd128; `Quat::mul` and the 4-lane dot in `Quat::normalize` associate differently…" | **confirmed** for wasm-scalar | unchanged; add: "On aarch64 glam uses NEON, a third variant: `Quat::mul` is bit-identical to SSE2, but the 4-lane dot (`(x+y)+(z+w)`), the reciprocal-multiply in `normalize` and the fused cross in `Quat * Vec3` differ." |
| §0 bullet 3 | option (a) `scalar-math` changes native output for every table, very-high, not recommended | **confirmed**; extended | add: "`scalar-math` on aarch64 alone equals x86_64 scalar-math, not SSE2 (S7 §5.5), so it cannot give aarch64 parity without moving x86_64." |
| §6.3 "Limits of (c)", 1st sentence | "parity is with **x86_64** native." | **confirmed** | unchanged. |
| §6.3 "Limits of (c)", 2nd sentence | "aarch64 native (macOS) already differs today (`ball_seam_bitident.rs:248-251`; glam's NEON path uses FMA `vfmaq_f32` in `mul_quat` and `vaddvq_f32` sums, `neon.rs:62-65`, `neon/quat.rs:916-918`)" | **changed** (differs: confirmed on qemu and on the macOS CI runner; cause partly wrong) | "aarch64 native differs today (fixture `ebfb6f43c9a0b859` on the macOS CI runner and under qemu; orientation only). The FMA in NEON `mul_quat` (`neon/quat.rs:916-918`) multiplies by ±1.0 and is exact; the causes are the `vaddvq_f32` 4-lane sum (`neon.rs:62-65`), `Vec4::normalize` by reciprocal multiply (`neon/vec4.rs:644-649`) and the fused `vfmsq_f32` cross product (`neon/vec3a.rs:293`) inside `Quat * Vec3`." |
| §6.3 "Limits of (c)", 3rd sentence | "browsers on ARM run the same wasm and match x86_64 (wasm SIMD semantics are IEEE-exact on every CPU; not measured on ARM hardware here)" | **confirmed by measurement (emulated)** | "Browsers on ARM run the same wasm and match x86_64: measured on V8's ARM64 backend (Node 22.23.5 linux-arm64 under qemu-aarch64), 11/11 scenarios per tick under default tiering, Liftoff-only and TurboFan-only; JIT code shows no fused multiply-add. Not measured on ARM hardware, JavaScriptCore or SpiderMonkey." |
| §6.3 "Limits of (c)", 4th sentence | "Requires Wasm fixed-width SIMD in the browser…" | **changed** (only without S7's change) | "With S7's `quat_ops` change, the default (no `simd128`) wasm build also matches native x86_64 (11/11), so parity no longer requires Wasm SIMD; `simd128` remains a speed choice." |
| §11 (i), 1st–2nd sentence | "Build the web sim with `-C target-feature=+simd128` …, keep native unchanged, and make a native-vs-wasm per-tick hash comparison … a failing CI gate" | **changed** | "Land `pin_physics::quat_ops` (S7 patch 0006; x86_64 output unchanged, aarch64 output changes: very-high blast radius under D34), build the web sim with or without `simd128`, and gate x86_64 = aarch64 (qemu on PRs, macOS on main) = wasm on the reference replay and seam digests (amended D9)." |
| §11 (i), clippy/±0 sentence | "the ±0 tie rule is the one known op-level difference under simd128; add those methods to … `disallowed-methods`" | **extended** | add: "Natively, aarch64 `f32::min/max` also break ±0 ties differently from x86_64 and wasm (`fmaxnm`: `max(+0,-0)=+0` vs `-0`); no run reached one, but only the aarch64 gate guards it." |
| §11 (i), R-4 | "make `Fnv64::write_f32` hash every NaN as one canonical pattern … removes the only platform-dependent bit pattern" | **confirmed, stronger** | add: "Generated NaNs are `7fc00000` on aarch64 native and on ARM64 V8 and `ffc00000` on x86_64 native and x64 V8, so NaN bits also differ between browsers on different CPUs." |

## 9. Reproduction (commands re-run to the same hashes)

```sh
. S7/env.sh                                   # COMMON env + aarch64 linker/runner
S7/scripts/flavour.sh base                    # or: portable (0006 + probe) | scalar (experiment i)
cd S7/repo && cargo build --release --target aarch64-unknown-linux-gnu -p s1_harness --features native --bins
cargo build --release -p s1_harness --features native --bins && cargo build --release -p pin_sim --bin pin_sim
cp the binaries to S7/bin/<flavour>/ (aarch64_base, aarch64_portable, aarch64_scalar, x86_64_base, x86_64_portable;
#  deleted in cleanup, rebuild them; pin_sim for aarch64: cargo build --release --target aarch64-unknown-linux-gnu -p pin_sim --bin pin_sim)
mkdir -p S7/dl/node_arm64/x && tar -xJf S7/dl/node_arm64/node-v22.23.5-linux-arm64.tar.xz -C S7/dl/node_arm64/x node-v22.23.5-linux-arm64/bin/node
S7/scripts/native_arm.sh aarch64_base          # §3: 11 FAIL lines, first ticks 6/86/28/8/20
S7/scripts/native_arm.sh aarch64_portable      # §5.2: 11 PASS
S7/scripts/native_arm.sh x86_64_portable x86   # §5.3 (b): 11 PASS
S7/scripts/lua_record_arm.sh aarch64_base      # §3: 33 PASS (real Lua on aarch64, byte-identical streams)
qemu-aarch64 -L /usr/aarch64-linux-gnu S7/bin/aarch64_<f>/pin_sim S7/repo/crates/pin_sim/tests/fixtures/replay_table --ticks 10000
S7/bin/x86_64_portable/pin_sim S7/repo/crates/pin_sim/tests/fixtures/replay_table --ticks 10000   # 25097c21c2c3adc1
cargo test -p pin_physics [--test ball_seam_bitident -- --nocapture]                  # x86_64, §5.3 (c)
cargo test --target aarch64-unknown-linux-gnu -p pin_physics --test ball_seam_bitident --no-run; qemu-aarch64 -L /usr/aarch64-linux-gnu <bin> --nocapture
S7/scripts/wasm_arm.sh simd default | simd liftoff --liftoff-only | simd turbofan --no-liftoff | base default   # §7
S7/scripts/wasm_run.sh <pkg_node> <label> x86|arm                                    # §5.3 (d)
s1 probe glam_ops | glam_iso | quat_ops_iso | fp_ties | nan_bits                     # §3, §4.1, §5.2
cd S7/mini && CARGO_TARGET_DIR=…/target/web_s7/mini cargo build --release [--target aarch64-unknown-linux-gnu] [--features scalar-math|glam-libm] ; python3 -I S7/scripts/mini_table.py S7/probes/mini <variants>   # §4.3
git -C /home/user/kilohertz-pinball apply --check S7/patches/0006-pin_physics-portable-quat-ops.patch
```

## 10. Residual risks

* **Emulation, not hardware, for the patched build.** qemu TCG implements each AArch64 FP instruction with softfloat to
  the Arm ARM; for the *unpatched* build it reproduced the Apple Silicon CI values exactly (§4.5). The *patched* build
  has not run on Apple hardware; the proposed `macos-latest` gate closes that on the first push.
* **Not glam-free:** the sim still uses glam `Vec3` (scalar on every target, identical) and `Mat4::transform_point3` at
  load (identical). A future use of a SIMD-backed glam type (`Quat` operators directly, `Vec3A`, `Vec4`, `Affine3A`,
  `lerp/slerp`) in a sim crate would bring the divergence back; the aarch64 gate catches it in the reference set only.
  The follow-up should add `disallowed-methods`/`disallowed-types` for those in the sim crates' `clippy.toml` (operator
  `*` on `Quat` cannot be linted by name: review rule + gate).
* **Outside glam:** `f32::min/max` ±0 ties and NaN bit patterns differ between aarch64 and x86_64 (§3); not reached in
  any measured run, guarded only by the gate. Lua `math.*` (`sin`, `exp`, `pow`, …) in **user** bundles calls the
  platform C libm (glibc, Apple, wasm libc): the three included tables do not use `math` (grep), so not measured;
  glibc's results are identical on x86_64 and aarch64 but differ from the `libm` crate in 1.5 % of inputs
  (`probes/trig_*`), and Apple's libm is unknown. A user table using `math.sin` can diverge across OSes (S2/web Lua item).
* **Tests that build inputs with glam/std trig:** `ball_seam_bitident`'s `slope` needs `quat_ops` for its inputs before
  its digest can be asserted on aarch64 (§5.2); `spec_ball_common`'s pitched world is built the same way (glam
  `from_rotation_x` + `rot * Vec3`) and already matches with 0006, but should be converted too (std `sin_cos` = Apple
  libm on macOS).
* **Browsers:** JavaScriptCore/SpiderMonkey on ARM and V8 on real ARM hardware not measured (needs S5/web/device).
* Release packages for macOS x86_64 run under Rosetta 2 on arm64 (release.yml:139); Rosetta's SSE emulation is
  expected to be exact but was not measured.


## 11. Effort for the follow-up (very-high blast radius under D34)

1. **pin_physics `quat_ops` (0006) + test inputs** — land 0006, convert `ball_seam_bitident::slope_scenario` and
   `spec_ball_common`'s pitched world to `quat_ops` + `libm::sinf/cosf` (x86_64 EXPECTED unchanged, measured), widen
   `RECORDING_TARGET` to aarch64, add `disallowed-types`/`-methods` for SIMD-backed glam types in the sim crates'
   `clippy.toml`. Very-high blast radius (sim output on macOS arm64 changes) → implementer **Opus 5.5** plus the
   independent tester and CodeRabbit; Fable 5.2 not needed (the change is small and every effect is measured here),
   only if the Orchestrator's D42 allowance check says yes. ≈ 2–3 agent-runs.
2. **CI gate + D9 amendment** — qemu-aarch64 leg in `ci.yml` `replay-hash` (PRs) and golden assertions in the
   `macos-latest`/`windows-latest` `replay-hash-cross` legs; PLAN §2.4 / decision-log text from §6 (owner-visible
   decision, D34 high). **Sonnet 5.5** for the YAML, the D9 text by the Orchestrator. ≈ 1–2 agent-runs.
3. **R-4 canonical NaN in `Fnv64::write_f32`** (no golden moves; S1) — **Sonnet 5.5**, 1 agent-run.

Total ≈ 4–6 agent-runs.

## 12. Cleanup and tree state

* `S7/repo` is left in the **`portable`** flavour (0006 + the S7 harness probes; `S7/repo/.s7_flavour`);
  `scripts/flavour.sh base|scalar` switches it back.
* `rm -rf /home/user/kilohertz-pinball/target/web_s7` done (including the `mini` and `wasm` sub-dirs); built binaries
  (`S7/bin`) and the extracted Node binary deleted; the verified Node tarball and SHASUMS kept in `S7/dl/node_arm64/`.
* Per-tick hash files: the 11 unique ones (`hashes/aarch64_base/*.hashes.gz`) kept gzip'ed; the 132 others were
  byte-identical to an S1 reference file and were replaced by `hashes/MANIFEST.txt` (sha256 + which S1 file each equals).
  qemu `-d in_asm` logs gzip'ed (`logs/jit/*.log.gz`, 5.4 MB in total).
* `git -C /home/user/kilohertz-pinball status --porcelain` → empty (only the ignored `target/` exists, now without
  `web_s7`); only read-only git commands touched the real tree (`log -1`, `rev-parse`, `remote -v`, `status`,
  `apply --check`). `S7/stage/b/` holds the patched pin_physics files the 0006 diff was generated from.
