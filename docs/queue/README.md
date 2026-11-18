# Human queue

One file per unit of work (D41): items for a person to look at (PLAN.md §2.3 rule 8, §2.6
failure policy).

- **File name:** `docs/queue/<unit-id>.md`, with the unit id as used in the branch name and run log.
- **Lines:** `- [ ] [<id>] text` for an open item, `- [x] [<id>] text` for a resolved one. The
  id in brackets and non-empty text are required (`^- \[( |x)\] \[[^\]]+\] \S`). The older
  styles `- [id] text` and `- ~~...` are also accepted.
- **Later edits:** when a unit edits or resolves an existing queue item, it edits that item's own
  file in `docs/queue/`, not another file.
- **Frozen history:** `docs/HUMAN_QUEUE.md` is frozen and is never edited, not even to strike a
  line through. A unit that resolves or updates an item from it adds one of these lines to its own
  `docs/queue/<unit-id>.md`:
  - an update that still awaits a ruling:
    `- [ ] [<original id>] update: <what changed> (frozen HUMAN_QUEUE.md item: "<short quote>")`;
  - an actual resolution:
    `- [x] [<original id>] resolved: <how> (frozen HUMAN_QUEUE.md item: "<short quote>")`.

  Both forms match the checkbox regex above. To find how a frozen item was resolved, grep `docs/queue/` for its id.
