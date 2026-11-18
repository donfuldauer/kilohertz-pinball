## fix-12-telemetry-session-id — telemetry session id deferred to protocol v2

**Decision:** Keep telemetry protocol v1 without a session id. The limitation is documented in
`docs/TELEMETRY_PROTOCOL.md` §6: a delayed full state that arrives after a later one is
indistinguishable from a sender restart. The session id is deferred to a version 2 header.

**Rationale:** A session id changes the header layout, which is a breaking v2 change. It needs a
`pin_core` contract change request (PLAN §2.3 rule 2) and updates to `pin_core` and `pin_audio`.
v1 recovers on the next full state, which the sender schedules every 1000 ticks, so the cost of
the missing id is a bounded delay and not a permanent desync.

**Revisit when:** a v2 protocol bump is made for another reason, or reconnect bugs are observed in
practice (a listener mistaking a delayed full state for a sender restart, or the reverse).
Source: review thread https://github.com/donfuldauer/kilohertz-pinball/pull/3#discussion_r4573605021
and GitHub issue #12.

**Alternatives:** Add the session id now (rejected: a breaking change with no observed bug).
