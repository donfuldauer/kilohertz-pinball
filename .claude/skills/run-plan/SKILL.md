---
name: run-plan
description: "The Orchestrator's loop over the Kilohertz Pinball plan's GitHub issues: finds every ready work-unit issue (all dependencies merged), works each one with the work-issue skill (in parallel where owned paths do not overlap), drives PRs to merge, runs phase gates, and keeps the run log current. Use to start or resume unattended work across many issues (/run-plan, optionally with a tracking issue or phase)."
---

# run-plan

You are the **Orchestrator** (PROCESS.md P5). You route, spawn, open PRs, merge and log. You write
no product code and review no code.

Repository: `donfuldauer/kilohertz-pinball`. GitHub through MCP tools (with `gh api -X GET
search/issues` paged explicitly for label searches) or through the `gh` CLI when MCP tools are not
configured (PROCESS.md P1 "Working with GitHub"). If there are no `work-unit` issues, stop and use
the `plan-from-spec` skill instead.

## 0. Resume

1. `git fetch origin` and check out the latest `main`.
2. Read `docs/RUN_LOG.md` (last rows), `docs/HUMAN_QUEUE.md` (anything a person has answered
   since the last run: struck-through P8 lines with a resolution), and the open `work-unit`
   issues' phases and dependencies.
3. List open PRs you created (`list_pull_requests` or `gh pr list`). Each open PR is driven to
   merge (step 4) before new work on the same paths starts, except the PR of an issue labelled
   `status:blocked-failure`: it waits for a person and reserves no paths (§1).
4. Set `FABLE_ALLOWANCE` (Claude Code) or T3 availability (Gemini / Antigravity) for this session's
   classifier calls (PROCESS.md P4): `insufficient` if the session reports a current rate limit,
   throttling or fallback at T3; otherwise `unknown`. Re-check it before any call that could route
   T3. In Claude Code, it is `sufficient` only when the session can read the account's usage
   limits and they leave enough Fable allowance; unknown means no Fable, and the run never asks
   the owner or waits. On Gemini / Antigravity, it is `sufficient` when the active plan and quota support
   T3 and no rate limit, throttling or fallback is reported.

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
reserve nothing. Overlapping claims are settled by `work-issue` §1 step 4.3. Go through the
candidates in plan order (ascending `KP-NNN`): start a candidate if its owned paths overlap no
reservation, and reserve its paths before considering the next candidate (so of two overlapping
ready issues the earlier one starts). Stop at a sensible parallelism (default 3 at once; the
agent environment has 4 cores, `docs/DEVELOPMENT.md`).

## 2. Per issue

Run the `work-issue` skill for it, from §1 (dependency check and claim) to §7 (merge and
unblock). Each issue works in its own git worktree (`../kilohertz-issue-<N>`), so parallel
issues never share a working tree. Keep going with other ready issues while a PR waits for CI
or a gate review. Run at most one gate review at a time (PROCESS.md P6): a second gate issue
waits, with its PR open, until the first gate closes out. Within one phase, a ready PR waits while
a lower-numbered issue of the phase has an open PR (unless that issue depends on the ready PR's
issue, as a gate issue waiting for its blocking fix does; a `status:blocked-failure` issue never holds a PR waiting, P6 "Gate issue"; it still counts for gate detection), and the gate test is re-run immediately
before each merge (PROCESS.md P6 "Gate issue"), so the phase's last issue always gets its gate review.

## 3. Discovered work

Deferred findings, requests outside owned paths and gaps found on the way become new `work-unit`
issues with their dependencies, labels and tracking issue (sub-issue of #79 when M1 needs them,
else #80); nothing is added to the repository for them except, where a judgement call was made,
its `docs/DECISIONS.md` entry. Only P8 items get a `docs/HUMAN_QUEUE.md` line.

## 4. After each merge

Merges make new issues ready (`work-issue` §7 step 3): go back to §1.

## 5. Phase gate

The phase's last open issue (an optional `status:blocked-failure` issue that P7 item 4 skips does
not count) is its gate issue: `work-issue` §6 runs its CodeRabbit gate review (cumulative since
the last reviewed commit in `docs/RUN_LOG.md`) before it merges, and the same review closes the
milestone when it is also the tracking issue's last open sub-issue; a blocking finding too large
for the gate branch becomes a new issue the gate issue depends on.

When every issue with a phase's `phase:` label is complete under the merged-PR rule (closed by a
merged PR, checked via `issue_read` `closed_by_pull_requests` and `pull_request_read` with the gh search fallback in `work-issue` §1 step 2, or replaced as `work-issue` §1 step 2 describes; or optional and blocked under P7),
run the phase gate from PROCESS.md P5 on `main`: the full three-OS CI matrix of the last push to
`main`, replay hashes, earlier phases' tests, the tick-time benchmark (method in
`docs/DEVELOPMENT.md`), and any automated gate the phase owns (A1, A2, A3). Green: record a
phase-gate row in `docs/RUN_LOG.md` with the commit SHA, the CI run and `reviewed <sha>` (the
reviewed head CodeRabbit approved, not the merge commit; PROCESS.md P6 "Gate reviews" step 7), committed with the next PR; no git tag (D39). Red: each failure becomes a new issue against the owning area; never
pause for a person.

## 6. When to stop

Stop when no issue is ready, no issue is `in-progress`, and no PR is open, or when every
remaining issue is blocked. End with a short account: phases completed, PRs merged, blocked
issues and why, `[Fable wanted]` notes from the run log, the state of Gates A1, A2 and A3, and the
P8 human-queue items a person should look at first. If M1 (#79) is complete, say that final human
acceptance (SPEC §4 Phase 8, `docs/HUMAN_ACCEPTANCE.md`) can be repeated.
