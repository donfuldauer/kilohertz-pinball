---
name: pr-watch
description: "Gemini / Antigravity PR watch-and-fix workflow (standing in for Claude Code's subscribe_pr_activity / auto-fix): schedules non-blocking check-ins on an open PR, inspects CI (every PR) and, on phase/milestone gate review PRs only, CodeRabbit reviews via gh, addresses every finding (fix, verify, reply with evidence, resolve conversations yourself), requests @coderabbitai review or @coderabbitai approve when needed, and runs to completion until PR checks are green and, for a gate, CodeRabbit approves (docs/PROCESS.md P6)."
---

# pr-watch (Gemini / Antigravity)

This skill is the **Gemini / Antigravity** equivalent of Claude Code's PR auto-fix /
`subscribe_pr_activity` workflow (`docs/PROCESS.md` P6). Because Antigravity does not have a
built-in GitHub-webhook PR subscription, `pr-watch` drives an open pull request to **green CI checks**
(and, for a phase or milestone gate, **CodeRabbit approval** of its gate review PR) through
bounded, non-blocking check-ins, resolving every review thread itself once justified. Routine
unit-of-work PRs get no CodeRabbit review (`docs/PROCESS.md` P6).

Repository: `donfuldauer/kilohertz-pinball`. Use the official `gh` CLI (or GitHub MCP tools if
configured; `docs/PROCESS.md` P1). Never sleep-poll in a foreground loop.

## Inputs

- Pull request number `<P>` (if omitted, detect the open PR for the current branch with
  `gh pr view --json number,url,headRefName,headRefOid`).
- Whether to merge once ready (`work-issue` §7 merges automatically when all P6 merge conditions
  hold; standalone invocations stop and report once all checks are green and, for a gate,
  CodeRabbit approved, unless the user explicitly asked to merge).

## 1. Inspect the PR state

Run a fresh read of the PR head, labels, checks, reviews, issue comments, and **all** review
threads:

1. **PR metadata, labels, checks and reviews:**
   ```sh
   gh pr view <P> --json number,url,state,headRefName,headRefOid,labels,reviewDecision,statusCheckRollup,reviews,comments
   ```
   Determine whether CodeRabbit review is required (`docs/PROCESS.md` P6 "Gate reviews"; retired
   `review:skip` / `review:sanity` / `review:full` labels are ignored):
   - **Gate:** `<P>` is a gate review PR (its base branch starts with `review-base/`), or `<P>`
     closes a gate issue (labelled `review:gate`, or the last open `work-unit` issue of its
     `phase:*` label or of its tracking issue). Watch the gate issue's PR **and** every review PR of
     the gate together. The review PRs of the gate are the step-2 review PR (head branch = the
     issue PR's head branch) or, when the review was split (P6 "Review PR" step 3), each open area
     PR (base `review-base/<gate>`, head `review/<gate>-<area>`; list them with
     `gh pr list --base review-base/<gate> --state open --json number,headRefName,headRefOid,body`;
     the oversized review PR they replaced is closed and no longer counts). If the watcher was
     started on any review PR of the gate (the step-2 review PR or an area PR), first resolve the
     gate issue PR (the open PR into `main` whose head branch is the gate branch: for the step-2
     review PR, its own head branch, via
     `gh pr list --base main --head <headRefName> --state open`; for an area PR, the PR named in
     the area PR's body) and treat it as `<P>` from here on, so `<P>` is always the gate issue PR
     and its `headRefOid` is the gate head. CI is read from
     `<P>`'s shared head commit (`headRefOid`); CodeRabbit findings, threads and approval are read
     from each review PR (`<R>` below, in turn; for a non-gate PR, `<R>` is `<P>`), and, on a gate,
     always also from the gate issue PR `<P>`'s own review threads and review bodies, all
     accounted for before completion is reported. **The current
     head of a review PR:** for the step-2 review PR it is `<P>`'s `headRefOid`; for an area PR it
     is the area PR's own `headRefOid`, and it counts only while the gate-head SHA the area PR
     records as built-from (its description or its latest rebuild comment) equals `<P>`'s
     `headRefOid`; otherwise the area PR is stale (rebuild it as `work-issue` §6 step 2 says, then
     ask for review).
     If the gate review PR is not open yet, open it as `work-issue` §6 describes.
     - **Missed-gate backstop review** (P6 "Gate issue" (3)): when the review PR has head branch
       `review/<gate>-missed` (base `review-base/<gate>`), its gate issue's PR already merged, so
       no open gate issue PR exists and the resolution above does not apply. `<P>` is the backstop
       review PR itself; CI and the current head are read from its own `headRefOid` (the merge
       commit on `main` it was pushed at, plus any later merges of `main` into it). Its CodeRabbit
       findings, threads and approval are read from it as `<R>`. Fixes land on `main` through
       ordinary PRs (watched as routine PRs) that are then merged into the review branch; push
       nothing to the review branch otherwise. The last reviewed head is recorded as
       `reviewed <sha>`, and no phase-gate row is recorded until the review is approved. The
       gate-issue-PR flow above stays unchanged for every gate that has an open issue PR.
   - **Routine:** anything else. No CodeRabbit review is required or requested; findings are
     handled whenever any review or review comment exists, from any author.
2. **All review threads and comments of `<R>` (paginated via GraphQL so no thread or comment is
   missed; skip only on a routine PR whose step 1 data shows no review and no review comment from
   any author (`reviews` and `comments` both empty); on a gate, `<R>` is the review PR, and
   each review PR of the gate is read in turn; on a routine PR, `<R>` is `<P>`):**
   ```sh
   gh api graphql \
     -F owner=donfuldauer -F name=kilohertz-pinball -F pr=<R> -F after=null \
     -f query='
       query($owner: String!, $name: String!, $pr: Int!, $after: String) {
         repository(owner: $owner, name: $name) {
           pullRequest(number: $pr) {
             reviewThreads(first: 100, after: $after) {
               pageInfo { hasNextPage endCursor }
               nodes {
                 id isResolved path line
                 comments(first: 100) {
                   pageInfo { hasNextPage endCursor }
                   nodes { databaseId author { login } url body }
                 }
               }
             }
           }
         }
       }'
   ```
   Repeat with `-F after="<endCursor>"` while `reviewThreads.pageInfo.hasNextPage` is `true`.

   For any thread whose `comments.pageInfo.hasNextPage` is `true`, paginate its remaining comments
   before evaluating disposition:
   ```sh
   gh api graphql \
     -F threadId="<thread_id>" -F commentAfter="<endCursor>" \
     -f query='
       query($threadId: ID!, $commentAfter: String) {
         node(id: $threadId) {
           ... on PullRequestReviewThread {
             comments(first: 100, after: $commentAfter) {
               pageInfo { hasNextPage endCursor }
               nodes { databaseId author { login } url body }
             }
           }
         }
       }'
   ```
   Repeat with `-F commentAfter="<endCursor>"` while `comments.pageInfo.hasNextPage` is `true`.
3. Also inspect `reviews[].body` of `<R>` for additional / nitpick / outside-diff comments that do
   not have an inline thread, and its `comments` for CodeRabbit's summary and pre-merge check
   warnings (on a gate, `gh pr view <R> --json reviews,comments` for each review PR; on a routine
   PR, step 1's output).

## 2. Check completion

You are **done** when all applicable conditions hold on the current `headRefOid`:

1. **CI green:** every required check (specifically the required `pr-checks` `CheckRun` aggregator
   and any other required check) is **present** in `statusCheckRollup` on `headRefOid` and passes
   for its entry type (`status == "COMPLETED"` and `conclusion == "SUCCESS"` for `CheckRun` entries;
   `state == "SUCCESS"` for `StatusContext` entries; an empty or incomplete `statusCheckRollup`
   never satisfies this check), and every other entry in `statusCheckRollup` passes by the same
   entry-type rules:
   - for `CheckRun` entries (excluding path-gated `SKIPPED` runs, such as `spikes`, `release`, or
     `rules-wasm` when skipped by path filters), `status == "COMPLETED"` and
     `conclusion == "SUCCESS"`;
   - for `StatusContext` entries, `state == "SUCCESS"`.
2. **CodeRabbit approved on the current head (gates only, per §1):** on every gate review PR,
   `coderabbitai` has an effective `APPROVED` review covering that review PR's current head (§1)
   that has not been superseded by a later
   `CHANGES_REQUESTED` or `DISMISSED` review (a subsequent `COMMENTED` review does not retract an
   existing approval). On a routine PR this item is satisfied automatically.
3. **Every finding accounted for and every thread resolved (when review is required or any review
   comment exists):** every inline review thread has a disposition reply and `isResolved == true`,
   and every CodeRabbit review-body finding without an inline thread (outside-diff, additional, or
   summary items) has a recorded disposition (`docs/PROCESS.md` P6 "Gate reviews" step 4).
4. **Verification passed:** `headRefOid` has a passing `work-verifier` result (required for any
   merge under `docs/PROCESS.md` P6 and for any fix pushed in §3.A before resolving its
   thread; for a standalone report-only run that pushes no fixes and does not merge, passing the
   applicable test and conformance suites suffices when `work-verifier` was not run). If
   verification is missing on `headRefOid`, follow §3.C before proceeding to approval or merge
   steps.

When all applicable conditions hold:
- If invoked from `work-issue`, continue to `work-issue` §7 (merge and unblock).
- If invoked standalone and the user explicitly asked to merge, confirm every `docs/PROCESS.md`
  P6 merge condition holds on `headRefOid` — including a passing `work-verifier` result for
  `headRefOid` (test or conformance suites alone are not a substitute), dispositions for every
  CodeRabbit review-body finding without an inline thread (if reviewed), `main`'s last push CI
  green, and the branch up to date with `main` where required — and run
  `gh pr merge <P> --merge --match-head-commit <headRefOid>` (do not delete the
  branch; the repository deletes merged branches automatically).
- Otherwise (standalone without an explicit merge request), stop scheduling check-ins and report
  the PR URL, head SHA, routine or gate, approval state (gates only), resolved threads, and CI
  results to the user.

## 3. Act on findings, failures, missing approval, or missing verification

Handle the PR state in this order:

### A. Unresolved CodeRabbit findings or CI failures (one batch per round)

1. List every new or unresolved finding (inline threads, additional/nitpick items in review
   bodies, failed pre-merge checks, and failed CI jobs).
2. Route and fix under `docs/PROCESS.md` P5 "Batching", P6 "Gate reviews" step 4, and the
   `triage` skill §2 (gate fixes are committed on the shared head branch, so they land in the gate
   issue's PR too):
   - Defer non-blocking refactor and polish findings to a new `work-unit` issue in a later phase
     or milestone; never defer security (R11), verified correctness, R3/R4 or contract defects.
   - Unclear validity or cause → `issue-investigator` first.
   - Obviously small T0/T1 fixes → `triage` fast path.
   - Everything else (complex, cross-module, contract, sim-output, security) → `work-classifier`
     → `task-executor`.
   - Failed pre-merge checks (such as PR description checks) → update the body of the PR that
     reported the check (on a gate, a review PR `<R>`; on a routine PR, `<P>`) with
     `gh pr edit <R> --body ...`.
3. Batch all fix commits locally, update `docs/RUN_LOG.md` (and regenerate `docs/HUMAN_ACCEPTANCE.md`
   if needed), then commit the fix together with those updates. Re-run `work-verifier` on that exact
   commit, whether or not the run will merge, so the verified commit is the PR head
   (`docs/PROCESS.md` P6 routine step 1); if `work-verifier` returns `fail`, fix the failure and
   re-verify before pushing or resolving threads. Once `work-verifier` returns `pass`, push once to
   the PR branch; on a gate, then post `@coderabbitai review` on each review PR so CodeRabbit
   reviews the fix round (auto-review is off).
4. **Reply to every review thread and give every finding a disposition** (reason and evidence
   first: commit SHA + test output, SPEC / PROCESS / DECISIONS citation, or link to a new deferred
   `work-unit` issue / DECISIONS entry). Post each reply on the PR that contains the finding: on a
   gate that is the review PR `<R>` the finding came from, or the gate issue PR `<P>` when the
   finding is in `<P>`'s own threads or review body; on a routine PR, `<P>`. Below, `<F>` is that
   PR:
   ```sh
   gh api -X POST repos/donfuldauer/kilohertz-pinball/pulls/<F>/comments/<comment_id>/replies \
     -f body="<Fixed / Not a defect / Deferred>: <evidence and commit SHA or link>"
   ```
   Where a finding has no inline thread (e.g., an additional comment in the review body), include
   its disposition in a concise top-level comment on `<F>` (`gh pr comment <F> --body ...`).
5. **Resolve every review thread yourself** once justified (`docs/PROCESS.md` P6 "Gate reviews"
   step 4, D37) — do **not** wait for CodeRabbit to auto-resolve threads and never ask a person to resolve one:
   ```sh
   gh api graphql -f id="<thread_id>" -f query='
     mutation($id: ID!) {
       resolveReviewThread(input: {threadId: $id}) {
         thread { isResolved }
       }
     }'
   ```
   **Never resolve** a thread that reports an unfixed R11 security issue or a verified unfixed
   correctness defect; fix and verify it first.
6. After pushing fixes and resolving threads, schedule the next check-in (§4) so CI (and, on
   a gate, CodeRabbit's requested re-review) can complete on the new head SHA.

### B. Checks or CodeRabbit are still pending or `IN_PROGRESS`

If any required check from §2 item 1 has not yet appeared in `statusCheckRollup`, or if any CI
check (or, on a gate, the CodeRabbit check on a review PR) is still pending, queued,
or `IN_PROGRESS` (and no check has failed), wait via a non-blocking scheduled check-in (§4).

### C. Verification is missing for `headRefOid`

If CI checks on `headRefOid` satisfy §2 item 1, every finding (if any) is accounted for and every
thread is resolved, and no check is pending or `IN_PROGRESS`, but §2 item 4 is not yet satisfied
on `headRefOid` (whether or not CodeRabbit has approved yet):

- **Merge requested (`work-issue` or standalone with an explicit merge request), or standalone
  report-only run where a fix was pushed:** start `work-verifier` on `headRefOid`.
  - If `work-verifier` returns `pass`, re-evaluate §2 and §3 on `headRefOid` (proceeding to §2's
    merge or report steps on a routine PR or once CodeRabbit approved the gate, or to §3.D / §3.E if
    a gate's CodeRabbit review or approval is still needed).
  - If `work-verifier` returns `fail`, route and fix the failures under §3.A (commit, verify that
    exact commit with `work-verifier`, push, and schedule the next check-in in §4).
- **Standalone report-only run (no merge requested and no fixes pushed):** if neither `work-verifier`
  nor the applicable test and conformance suites have passed on `headRefOid`, run the applicable
  suites on `headRefOid`.
  - If they pass, re-evaluate §2 and §3 on `headRefOid` (stopping and reporting under §2 if review
    is not required (routine PR) or CodeRabbit already approved the gate, or continuing to §3.D / §3.E if a gate's CodeRabbit review or
    approval is still needed).
  - If they fail, route and fix under §3.A (which runs `work-verifier` on the fix commit before
    pushing and resolving any thread).

### D. CodeRabbit has not reviewed the current head commit (gates only)

Skip this section on a routine PR. Otherwise, if CI checks on `headRefOid` satisfy §2 item 1,
verification has passed (§2 item 4), and CodeRabbit is neither pending/`IN_PROGRESS` nor has
reviewed that review PR's current head (§1) (for example after a fix push, or rate limiting):

```sh
gh pr comment <R> --body "@coderabbitai review"
```

(Use `@coderabbitai full review` for the first pass on a newly opened review PR.) Ask at most three
times per head commit (`docs/PROCESS.md` P6 "Gate reviews" step 5). If it still does not review
after three asks, record the status in `docs/RUN_LOG.md`, stop, and inform the user. Otherwise
schedule the next check-in (§4).

### E. All findings resolved, checks green, and verification passed, but CodeRabbit has not approved yet (gates only)

Skip this section on a routine PR. Otherwise, if every finding is accounted for and
every thread is resolved, CI checks on `headRefOid` satisfy §2 item 1, verification has passed
(§2 item 4), CodeRabbit has finished reviewing that review PR's current head (§1) (no pending or `IN_PROGRESS` CodeRabbit
check), and the only remaining blocker is CodeRabbit's `CHANGES_REQUESTED` or withheld automatic
approval:

```sh
gh pr comment <R> --body "@coderabbitai approve"
```

Then schedule the next check-in (§4).

## 4. Schedule the next non-blocking check-in

Never run a tight foreground `while` sleep-poll loop. Choose the check-in mechanism available in
your current session:

1. **Antigravity App / CLI with the built-in `schedule` tool (`/schedule`):**
   Schedule a one-shot timer (`DurationSeconds: 180` while waiting on CodeRabbit or fast checks;
   `300`–`600` for longer CI legs) whose prompt re-runs `pr-watch` on PR `#<P>`, then end the turn
   so the conversation stays responsive.
2. **Antigravity remote-agent VM without a `schedule` tool:**
   Launch a **single** bounded asynchronous check-in command in the background via `run_command`
   (`WaitMsBeforeAsync: 500`, `NotificationTimeoutSeconds: 600`) that sleeps once for the check-in
   interval (e.g., 120–180 seconds) and then prints a single `gh pr view` + review-thread count
   snapshot, and immediately stop calling tools so the runtime wakes you asynchronously when the
   background task finishes:
   ```sh
   sleep 150 && gh pr view <P> --json headRefOid,statusCheckRollup,reviews --jq '{head: .headRefOid, pending_or_failed: [.statusCheckRollup[] | select((.__typename == "StatusContext" and .state != "SUCCESS") or (.__typename != "StatusContext" and (.status != "COMPLETED" or (.conclusion != "SUCCESS" and .conclusion != "SKIPPED")))) | {name: (.name // .context), status: (.status // .state), conclusion}], coderabbit_reviews: [.reviews[] | select(.author.login == "coderabbitai") | {state, submittedAt, commit: .commit.oid}]}'
   ```
   When the wakeup notification arrives, go back to §1 and act on the updated state. Repeat until
   §2 holds (or stop after a reasonable retry limit on stalled checks/rate limits and report to the
   user).
3. **Unattended monitoring across sessions (Antigravity App):**
   The owner can configure a persistent `/automation` job that periodically runs `pr-watch` on open
   PRs.
4. **Sessions with no scheduling mechanism:**
   Report the PR URL, head SHA, pending checks, and open findings to the user, then stop and ask
   the user to re-invoke `pr-watch` after the pending checks complete.

## Never

Sleep-poll in a foreground loop; resolve a thread without a disposition reply; resolve an unfixed
R11 security issue or a verified unfixed correctness defect; ask a person to resolve a review
thread; skip, ignore, or weaken a test to get CI green; push directly to `main`.
