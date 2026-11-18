## fix-11-telemetry-min-ring — unclamped constructor mechanism

**Decision:** `telemetry_channel` clamps the ring capacity to
`max(requested, MAX_DATAGRAMS_PER_TICK)` (5) and writes one best-effort
warning to stderr (a failed write is ignored) when it raises the value. The
old exact-capacity behaviour stays
available as `#[doc(hidden)] pub fn telemetry_channel_unclamped`, re-exported
from `telemetry`, which the public function delegates to.

**Rationale:** The capacity-2 spec tests are integration tests, so they can
only see public items. A doc-hidden `pub fn` keeps them running unchanged
with no new feature flag or dependency, and the clamp and warning run only at
construction time, never on the sim thread's tick path.

**Alternatives:**
- A cargo feature (`cfg(feature = "test-support")`): needs a Cargo.toml change
  outside this unit's paths and extra CI wiring.
- `pub(crate)`: not visible to integration tests.
- Rejecting small values with an error: breaks the infallible constructor
  signature; clamping was the recommendation in issue #11.

## fix-11-telemetry-min-ring — capacity policy outside the versioned sections

**Decision:** The ring-capacity policy lives in the non-normative section 7
(implementation notes) of `docs/TELEMETRY_PROTOCOL.md`. Section 4 is unchanged
from `main`. No version bump.

**Rationale:** The clamp is a `pin_audio` sender implementation policy with no
wire or receiver change. The protocol preamble versions sections 2-4, so a v2
bump for this would be a breaking contract change with no wire difference.

**Alternatives:**
- Bump to version 2 via `contracts/CHANGE_REQUESTS.md`: rejected, there is no
  wire change.
- Keep the policy in section 4 unversioned: rejected, it violates the
  preamble.
