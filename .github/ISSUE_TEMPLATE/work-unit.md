---
name: Work unit
about: One reviewable unit of Kilohertz Pinball work (one PR). Use for new work found during a unit.
title: "KP-NNN: <title>"
labels: ["work-unit", "blocked"]
---

> **Plan ID:** KP-NNN · **Phase:** `<NN-name>` · **Milestone:** <M1 — Play experience | M2 — Backlog> (tracking issue #<79 | 80>) · **Expected tier:** <T1 (Sonnet 5.5) | T2 (Opus 5.5)>

<!-- After creating the issue: attach it as a sub-issue of its tracking issue, add the labels
     type:<feature|fix|infra|docs|content>, area:<…>, phase:<NN-name>, tier:<T1|T2>, and replace
     `blocked` with `ready` if every dependency is already satisfied (docs/PROCESS.md P6 step 5).
     No third-party pinball names anywhere in this issue. -->

## Summary

<what this unit delivers and why, in two or three sentences>

## Depends on

- #<n> KP-NNN <title>   <!-- or "None: can start immediately" (then label `ready`) -->

## Requirements

- REQ-<AREA>-<NNN> (`docs/SPEC.md` §<n>)   <!-- or a SPEC section or subphase, e.g. "SPEC §4 Subphase 2.5, Verification 1" -->

## Deliverables

- [ ] …

## Acceptance criteria

- [ ] …
- [ ] The unit gate in `docs/PROCESS.md` P5 passes.

## Owned paths

- `…`

## Notes

<constraints, related decisions (DECISIONS.md section titles), out of scope>

---
To work this issue, start a Claude Code session on this repository and say **"Start work on issue #<n>"** (`work-issue` skill).
