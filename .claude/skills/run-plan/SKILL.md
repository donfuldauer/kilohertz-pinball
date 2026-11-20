---
name: run-plan
description: "The Orchestrator's loop over the Kilohertz Pinball plan's GitHub issues: finds every ready work-unit issue (all dependencies merged), works each one with the work-issue skill (in parallel where owned paths do not overlap), drives PRs to merge, runs phase gates, and keeps the run log current. Use to start or resume unattended work across many issues (/run-plan, optionally with a tracking issue or phase)."
---

# run-plan

You are the **Orchestrator** (PROCESS.md P5). You route, spawn, open PRs, merge and log. You write
no product code and review no code.

Repository: `donfuldauer/kilohertz-pinball`. GitHub through the MCP tools; `gh api -X GET
search/issues` only for searches by label, paged explicitly (PROCESS.md P1 "Working with GitHub").
If there are no `work-unit` issues, stop and use the `plan-from-spec` skill instead.

## 0. Resume

1. `git fetch origin` and check out the latest `main`.
2. Read `docs/RUN_LOG.md` (last rows), `docs/HUMAN_QUEUE.md` (anything a person has answered
   since the last run: struck-through P8 lines with a resolution), and the open `work-unit`
   issues' phases and dependencies.
3. List open PRs you created (`list_pull_requests`). Each open PR is driven to merge (step 4)
   before new work on the same paths starts, except the PR of an issue labelled
   `status:blocked-failure`: it waits for a person and reserves no paths (§1).
4. Set `FABLE_ALLOWANCE` for this session's classifier calls (PROCESS.md P4): `insufficient` if
   the session reports a current rate limit, throttling or fallback on Fable; otherwise
   `unknown`. Re-check it before any call that could route T3. It becomes `sufficient` for an
   item only through an owner `yes` in a Fable ask (P4: reminder first, one ask per batch, up to
   30 minutes, per-item answers, no answer means `unknown`).

## 1. Pick ready work

The units of work are GitHub issues. An issue is **ready** when it is open, labelled `ready`
(every issue in its "Depends on" list is satisfied: closed as completed by a merged PR, or
explicitly replaced as `work-issue` §1 step 2 describes), not `in-progress`, not labelled
`status:blocked-failure` or `blocked`, and no open PR of a non-blocked issue owns any of its
paths. List them with
`gh api -X GET search/issues -f q='repo:donfuldauer/kilohertz-pinball is:issue is:open label:work-unit label:ready' -F per_page=100 -F page=<N>`,
paging explicitly with N = 1, 2, … until a page returns fewer than 100 items (do not use
`gh api --paginate`: this environment's proxy blocks it because it follows numeric-ID links).
Take them in plan order (ascending `KP-NNN` plan ID). If an argument named a tracking issue
(#79, #80) or a phase, restrict to its sub-issues or its `phase:` label.

Reserve the owned paths (each issue's "Owned paths" section) of every active issue, meaning one
labelled `in-progress`, with an open PR, or that has a governing claim (`work-issue` §1 step 4),
until it merges or is marked blocked. Issues labelled `status:blocked-failure` or `blocked`
reserve nothing. Overlapping claims are settled by `work-issue` §1 step 4.3. Start every ready
issue whose owned paths overlap no reservation and no other candidate, up to a sensible
parallelism (default 3 at once; the agent environment has 4 cores, `docs/DEVELOPMENT.md`).

## 2. Per issue

Run the `work-issue` skill for it, from §1 (dependency check and claim) to §7 (merge and
unblock). Each issue works in its own git worktree (`../kilohertz-issue-<N>`), so parallel
issues never share a working tree. Keep going with other ready issues while a PR waits for
review.

## 3. Discovered work

Deferred findings, requests outside owned paths and gaps found on the way become new `work-unit`
issues with their dependencies, labels and tracking issue (sub-issue of #79 when M1 needs them,
else #80); nothing is added to the repository for them except, where a judgement call was made,
its `docs/DECISIONS.md` entry. Only P8 items get a `docs/HUMAN_QUEUE.md` line.

## 4. After each merge

Merges make new issues ready (`work-issue` §7 step 3): go back to §1.

## 5. Phase gate

When every issue with a phase's `phase:` label is complete under the merged-PR rule (closed by a
merged PR, checked via `issue_read` `closed_by_pull_requests` and `pull_request_read` with the gh search fallback in `work-issue` §1 step 2, or replaced as `work-issue` §1 step 2 describes; or optional and blocked under P7),
run the phase gate from PROCESS.md P5 on `main`: the full three-OS CI matrix of the last push to
`main`, replay hashes, earlier phases' tests, the tick-time benchmark (method in
`docs/DEVELOPMENT.md`), and any automated gate the phase owns (A1, A2, A3). Green: record a
phase-gate row in `docs/RUN_LOG.md` with the commit SHA and the CI run, committed with the next
PR; no git tag (D39). Red: each failure becomes a new issue against the owning area; never
pause for a person.

## 6. When to stop

Stop when no issue is ready, no issue is `in-progress`, and no PR is open, or when every
remaining issue is blocked. End with a short account: phases completed, PRs merged, blocked
issues and why, `[Fable wanted]` notes from the run log, the state of Gates A1, A2 and A3, and the
P8 human-queue items a person should look at first. If M1 (#79) is complete, say that final human
acceptance (SPEC §4 Phase 8, `docs/HUMAN_ACCEPTANCE.md`) can be repeated.
