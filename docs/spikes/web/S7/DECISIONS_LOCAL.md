# web-spike-arm-parity (S7) — local decisions (rule 8; scratch only, not the repo)

## s7-qemu — aarch64 measured with qemu-user, not hardware

**Decision:** All aarch64 numbers come from `qemu-aarch64` 8.2.2 (user mode, TCG) running
binaries cross-compiled for `aarch64-unknown-linux-gnu` (rustc default CPU `generic`, features
`neon` only). The macOS target `aarch64-apple-darwin` (default CPU `apple-m1`) runs the same
glam NEON source (glam selects it by `target_arch = "aarch64"`), so the arithmetic is the same;
the instruction scheduling may differ, which does not change IEEE results without contraction.

**Rationale:** No ARM hardware on this VM; the brief prescribes qemu. TCG emulates each AArch64
FP instruction with softfloat to the Arm ARM rules (fused ops with one rounding, FPCR default
round-to-nearest), so op-level bit results are what hardware gives, except for NaN payload
corner cases that do not occur in the runs (every NaN check passes).

**Alternatives:** a macOS CI run (not available to a spike: the owner's CI only); Rosetta
(x86 on ARM, not the question asked).

## s7-scenario-length — full lengths instead of prefixes

**Decision:** qemu-user turned out fast enough (native s1 ~ measured ticks/s in RESULTS §2), so
every scenario runs its full length (60 000 / 20 000 / 10 000 ticks; the 600 000-tick soak
too) rather than a 10 000-tick prefix. `scripts/truncate_scn.py` exists for shorter runs.

**Rationale:** Full-length equality is a stronger claim than a prefix and costs minutes.

**Alternatives:** prefixes (the brief's floor), kept only as a fallback.

## s7-route — the parity fix: a portable replica of glam's SSE2 order, used on every target

**Decision:** The prototype adds `pin_physics::quat_ops` (plain `f32` code reproducing glam
0.33.12's SSE2 results for `Quat` mul, `length_squared`, `normalize`, `* Vec3` and
`from_rotation_arc`) and routes the 11 `Quat` operation sites of pin_physics through it on
**every** target (no `cfg`). x86_64 output is unchanged because the replica is bit-exact to
glam SSE2 (0 mismatches over 1 000 000 inputs per op, plus the x86 goldens).

**Rationale:** One code path is tested by the existing x86_64 goldens on every PR (fixture,
seam digests): a mistake in the replica fails CI on Linux at once, while a `cfg(aarch64)`-only
route would be exercised only on ARM legs. The replica also makes wasm32 without `simd128`
equal, and removes the dependence on glam's per-target SIMD code for every op the sim uses.

**Alternatives:** `cfg(target_arch = "aarch64")` around the same calls (x86_64 code literally
unchanged, but the replica untested on x86 CI; measured equal output either way); glam
`scalar-math` on aarch64 (measured: equals x86 scalar-math, not x86 SSE2; rejected); a patched
glam fork via `[patch.crates-io]` (fixes NEON at the source but vendors a third-party crate:
larger blast radius and maintenance; rejected for now).

## s7-glam-ops-proof — which probe proves op-level parity for the fix

**Decision:** The brief asks for "`s1 probe glam_ops` all per-op digests equal to the SSE2 column"
with the winning patch. S1's `glam_ops` builds its inputs with glam's own `normalize()` and then
measures glam itself; the winning fix leaves glam untouched (it routes the sim's 11 `Quat` sites
through `pin_physics::quat_ops`), so on aarch64 `glam_ops` still differs for 13 of 15 rows by
design. Op-level parity of the *patched sim* is therefore proven with two isolating probes whose
inputs are built by plain scalar code (bit-identical on every target): `glam_iso` (26 ops incl.
the sim's `integrate_orientation`) and `quat_ops_iso` (the replica's 6 ops), each over 200 000
inputs, plus the 1 000 000-input `mini` probe; every op the patched sim uses equals the x86_64
SSE2 digest on aarch64. The raw `glam_ops` output on aarch64 is still reported.

**Rationale:** A probe whose inputs already differ cannot attribute a difference to an op (S1's
`quat_mul` row differs on aarch64 only because its inputs come from NEON `normalize`; with
identical inputs NEON `Quat` mul is bit-identical, 0 of 1 000 000). Making `glam_ops` itself pass
would need a glam fork (NEON backend rewritten to SSE2 order for every op family), which is a
larger change than the brief's "smallest change".

**Alternatives:** patch glam's NEON backend via `[patch.crates-io]` (rejected: vendors and
maintains a third-party crate; still leaves Vec4 min/max ties and Mat4::inverse to port).

## s7-seam-test-inputs — what ball_seam_bitident prints on aarch64 after the fix

**Decision:** Report the unmodified test (4 of 5 digests equal on aarch64 after 0006; `slope`
differs) and, as an experiment only, the same test with `slope_scenario` building its inputs via
`quat_ops` (5 of 5 equal, x86_64 unchanged). The test change is a follow-up item, not part of
0006.

**Rationale:** The remaining `slope` difference comes from the test's own input construction
(`rot * v(...)`, `Quat * Quat` with glam NEON), not from the sim; separating the two keeps the
sim patch minimal and shows exactly what the gate needs.

**Alternatives:** fold the test change into 0006 (larger patch, mixes test inputs with sim code).
