## w7pre-proptest-tripwire — proptest workaround removal is test-enforced

**Decision:** `tests/spec_ci/tests/spec_proptest_workaround.rs` reads the root `Cargo.toml` and
`Cargo.lock`. It fails if either `[profile.{dev,test}.package.proptest] debug-assertions = false`
stanza is present while the locked proptest version is not exactly 1.12.0, and also fails if the
stanzas are missing while it is 1.12.0. The failure message tells the reader to check whether the
negative-step `debug_assert` in `FloatUniform::sample` is fixed, then either remove both stanzas
(and confirm `crates/pin_physics/tests/proptest_f32_sampler.rs` passes without them) or bump the
pinned version in the test.

**Rationale:** Removing the workaround used to depend on a person remembering to check. A
proptest upgrade (Cargo.lock change) now trips a test in CI, so the question is asked exactly when
it becomes relevant, and the workaround cannot vanish silently either.

**Alternatives:**
- A calendar reminder: rejected, easy to miss.
- Removing the workaround now: rejected, 1.12.0 is the latest release and still has the bug.
