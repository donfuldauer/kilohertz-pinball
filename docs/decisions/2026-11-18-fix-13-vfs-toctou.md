## fix-13-vfs-toctou — new docs/THREAT_MODEL.md for accepted security risks

**Decision:** Create `docs/THREAT_MODEL.md`, because no threat-model document existed. Issue #13
asked for an entry in "the threat-model doc". The VFS time-of-check/time-of-use finding is recorded
there as accepted risk `TM-1`, with its revisit trigger and the fix to use then.

**Rationale:** One place for accepted security risks, each with a revisit trigger, so the trade-off
can be found and re-checked when the bundle loading model changes.

**Alternatives:**
- A section in `docs/PLAN.md`: rejected. PLAN is the build plan and a shared file.
- An entry in the frozen `docs/DECISIONS.md`: rejected. That file is frozen history (D41).
