# Decisions — pr68-feature-lab-libm (2026-11-19)

## pr68-feature-lab-libm — VUK eject angle via libm

**Decision:** The Feature Lab bundle builder (`build_feature_lab.rs`, `vuk_eject`) computes the
VUK eject angle with `libm::atan2f` and the kick with `libm::hypot` on `f64` cast to `f32`,
instead of the std `f32::atan2` / `f32::hypot`. The fix is made inside PR #68 by Orchestrator
ruling, although the file is 6.4-owned.

**Rationale:** std `atan2f`/`hypotf` call the platform glibc. CI's newer glibc returns a
correctly rounded `atan2f` one ulp below glibc 2.39, so CI generated a different VUK
`angle_deg` and the 7.1 soak played a different game (a glibc-version dependence the soak
found). The libm-only rule (PLAN §2.4, D9) forbids that. `libm::atan2f` and the `f64`
`libm::hypot` cast to `f32` reproduce the glibc <= 2.40 bits exactly (angle 0x4208bd8c, kick
0x3ed8a09f), so the generated bundle is byte-identical on this machine.

**Alternatives:**
- A separate PR for the fix. Rejected: only one branch can be pushed and the failure blocks
  PR #68's CI.
- `libm::hypotf`. Rejected: it is one ulp off the current output and would change the bundle.
