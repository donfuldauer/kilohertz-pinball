# Human queue

One file per unit of work (D41): items for a person to look at (PLAN.md §2.3 rule 8, §2.6
failure policy).

- **File name:** `docs/queue/<unit-id>.md`, with the unit id as used in the branch name and run log.
- **Lines:** `- [ ] [<id>] text` for an open item, `- [x] [<id>] text` for a resolved one. The
  older line style used in `docs/HUMAN_QUEUE.md` is also accepted.
- **Later edits:** when a unit edits or resolves an existing queue item, it edits that item's own
  file in `docs/queue/`, not another file.
- **Frozen history:** `docs/HUMAN_QUEUE.md` is frozen history. For items that live there,
  resolution may still strike the line through (`~~...~~ Resolved: ...`). That small, rare edit is
  the only permitted edit to that file.
