## w7pre-log-split — one file per decision and queue entry (user ruling D41)

**Decision:** New decision entries live in per-unit files `docs/decisions/<date>-<unit-id>.md`, and
new queue entries in per-unit files `docs/queue/<unit-id>.md`. `docs/DECISIONS.md` and
`docs/HUMAN_QUEUE.md` are frozen history as of 2026-11-18. Resolving a queue item edits that item's
own file. `docs/HUMAN_QUEUE.md` is never edited again: a unit that resolves or updates an item from it adds
`- [x] [<original id>] resolved: <how> (frozen HUMAN_QUEUE.md item: "<short quote>")` to its own
`docs/queue/<unit-id>.md`. The earlier draft allowed striking through frozen lines; dropped after review
(CodeRabbit r4608955222) so the frozen file is never edited. The format test runs in `spikes.yml`, which
triggers on `docs/decisions/**` and `docs/queue/**`.

**Rationale:** Shared append-only files made every merge conflict with every other open PR. The
re-sync merges and the full CI cycles they triggered serialised merges. The user approved this on
2026-11-18.

**Alternatives:** Keep appending with a `merge=union` gitattribute (rejected: union merges silently
hide conflicts); a generated index (rejected: an index is itself a shared file); splitting the old
entries into files (rejected: churn with no benefit).
