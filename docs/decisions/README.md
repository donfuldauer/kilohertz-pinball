# Decisions

One file per unit of work (D41). Every ambiguity an agent resolved goes here, with rationale
(PLAN.md §2.3 rule 8).

- **File name:** `docs/decisions/<YYYY-MM-DD>-<unit-id>.md`. The unit id is the one used in the
  branch name and the run log (for example `w7pre-log-split`); the date is the day the unit starts.
- **Shape:** `## <id> — <topic>` (a non-empty id, a space, an em dash U+2014, a space, a non-empty
  topic; checked by `^## (\S.*?) — (\S.*)$`), then **Decision**, **Rationale**, **Alternatives**, as in the
  older entries. One file may hold several `##` entries from the same unit.
- **Ownership:** never edit another unit's file, except to correct or supersede it with a new
  entry in your own file.
- **Unique names:** file names must be unique, and so must the unit id after the date.
- **Frozen history:** `docs/DECISIONS.md` is frozen history (D41) and is the only place older
  entries live.
- **Index:** the list is the directory itself (`ls docs/decisions`). There is no index file to
  maintain, because an index would be a shared file and bring the merge conflicts back.
