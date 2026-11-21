---
name: pr-watch
description: "Gemini / Antigravity PR watch-and-fix workflow (standing in for Claude Code's subscribe_pr_activity / auto-fix): schedules non-blocking check-ins on an open PR, inspects CI and CodeRabbit reviews via gh, addresses every finding (fix, verify, reply with evidence, resolve conversations yourself), requests @coderabbitai review or @coderabbitai approve when needed, and runs to completion until CodeRabbit approves and PR checks are green (docs/PROCESS.md P6)."
---

# pr-watch (Gemini / Antigravity)

This skill is the **Gemini / Antigravity** equivalent of Claude Code's PR auto-fix /
`subscribe_pr_activity` workflow (`docs/PROCESS.md` P6). Because Antigravity does not have a
built-in GitHub-webhook PR subscription, `pr-watch` drives an open pull request to **CodeRabbit
approval** and **green CI checks** through bounded, non-blocking check-ins, resolving every review
thread itself once justified.

Repository: `donfuldauer/kilohertz-pinball`. Use the official `gh` CLI (or GitHub MCP tools if
configured; `docs/PROCESS.md` P1). Never sleep-poll in a foreground loop.

## Inputs

- Pull request number `<P>` (if omitted, detect the open PR for the current branch with
  `gh pr view --json number,url,headRefName,headRefOid`).
- Whether to merge once approved (`work-issue` §7 merges automatically when all P6 step 5 merge
  conditions hold; standalone invocations stop and report once CodeRabbit approves and all checks
  are green, unless the user explicitly asked to merge).

## 1. Inspect the PR state

Run a fresh read of the PR head, checks, reviews, issue comments, and **all** review threads:

1. **PR metadata, checks and reviews:**
   ```sh
   gh pr view <P> --json number,url,state,headRefName,headRefOid,reviewDecision,statusCheckRollup,reviews,comments
   ```
2. **All review threads (paginated via GraphQL so no thread is missed):**
   ```sh
   gh api graphql \
     -F owner=donfuldauer -F name=kilohertz-pinball -F pr=<P> -F after=null \
     -f query='
       query($owner: String!, $name: String!, $pr: Int!, $after: String) {
         repository(owner: $owner, name: $name) {
           pullRequest(number: $pr) {
             reviewThreads(first: 100, after: $after) {
               pageInfo { hasNextPage endCursor }
               nodes {
                 id isResolved path line
                 comments(first: 20) {
                   nodes { databaseId author { login } url body }
                 }
               }
             }
           }
         }
       }'
   ```
   Repeat with `-F after="<endCursor>"` while `pageInfo.hasNextPage` is `true`.
3. Also inspect `reviews[].body` for additional / nitpick / outside-diff comments that do not have
   an inline thread, and `comments` for CodeRabbit's summary and pre-merge check warnings.

## 2. Check completion

You are **done** when all of these hold on the current `headRefOid`:

1. **CI green:** every entry in `statusCheckRollup` passes for its entry type:
   - for `CheckRun` entries (excluding path-gated `SKIPPED` runs), `status == "COMPLETED"` and
     `conclusion == "SUCCESS"` (specifically the required `pr-checks` aggregator plus any other
     triggered workflow such as `spikes`, `release`, or `rules-wasm`);
   - for `StatusContext` entries, `state == "SUCCESS"`.
2. **CodeRabbit approved on the current head:** the latest `coderabbitai` review covers
   `headRefOid` with `state == "APPROVED"`, with no outstanding `CHANGES_REQUESTED`.
3. **Zero unresolved threads:** every review thread has a disposition reply and
   `isResolved == true`.
4. **Verification passed:** the head commit passed `work-verifier` (or the applicable test and
   conformance suites for a standalone docs/config PR).

When all four hold:
- If invoked from `work-issue`, continue to `work-issue` §7 (merge and unblock).
- If invoked standalone and the user explicitly asked to merge, verify the remaining
  `docs/PROCESS.md` P6 step 5 merge conditions (`main`'s last push CI green and branch up to date
  with `main` where required) and run `gh pr merge <P> --merge --match-head-commit <headRefOid>`
  (deleting the branch afterward if it was not auto-deleted).
- Otherwise (standalone without an explicit merge request), stop scheduling check-ins and report
  the PR URL, head SHA, approval state, resolved threads, and CI results to the user.

## 3. Act on findings, failures, or missing approval

Handle the PR state in this order:

### A. Unresolved CodeRabbit findings or CI failures (one batch per round)

1. List every new or unresolved finding (inline threads, additional/nitpick items in review
   bodies, failed pre-merge checks, and failed CI jobs).
2. Route and fix under `docs/PROCESS.md` P5 "Batching" and the `triage` skill §2:
   - Unclear validity or cause → `issue-investigator` first.
   - Obviously small T0/T1 fixes → `triage` fast path.
   - Everything else (complex, cross-module, contract, sim-output, security) → `work-classifier`
     → `task-executor`.
   - Failed pre-merge checks (such as PR description checks) → update the PR body with
     `gh pr edit <P> --body ...`.
3. Update `docs/RUN_LOG.md` (and regenerate `docs/HUMAN_ACCEPTANCE.md` if needed), then commit the
   fix together with those updates. Re-run verification (`work-verifier`, or all applicable
   conformance and test suites for standalone docs/config PRs) on that exact commit so the verified
   commit is the PR head (`docs/PROCESS.md` P6 step 1), and push it to the PR branch.
4. **Reply to every review thread** (reason and evidence first: commit SHA + test output, SPEC /
   PROCESS / DECISIONS citation, or link to the new deferred `work-unit` issue / DECISIONS entry):
   ```sh
   gh api -X POST repos/donfuldauer/kilohertz-pinball/pulls/<P>/comments/<comment_id>/replies \
     -f body="<Fixed / Not a defect / Deferred>: <evidence and commit SHA or link>"
   ```
   Where a finding has no inline thread (e.g., an additional comment in the review body), include
   its disposition in a concise PR comment (`gh pr comment <P> --body ...`).
5. **Resolve every review thread yourself** once justified (`docs/PROCESS.md` P6 step 3, D37) —
   do **not** wait for CodeRabbit to auto-resolve threads and never ask a person to resolve one:
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
6. After pushing fixes and resolving threads, schedule the next check-in (§4) so CodeRabbit's
   incremental review and CI can complete on the new head SHA.

### B. All findings resolved and checks green, but CodeRabbit has not approved yet

If every thread is replied to and resolved, all CI checks on `headRefOid` are green, CodeRabbit has
finished reviewing `headRefOid` (no active `IN_PROGRESS` CodeRabbit check), and the only remaining
blocker is CodeRabbit's `CHANGES_REQUESTED` or withheld automatic approval:

```sh
gh pr comment <P> --body "@coderabbitai approve"
```

Then schedule the next check-in (§4).

### C. CodeRabbit has not reviewed the current head commit

If CI has finished (or a check-in has elapsed since the push) and CodeRabbit is neither
`IN_PROGRESS` nor has reviewed `headRefOid` (for example due to rate limiting):

```sh
gh pr comment <P> --body "@coderabbitai review"
```

Ask at most three times per head commit (`docs/PROCESS.md` P6 step 4). If it still does not review
after three asks, record the status in `docs/RUN_LOG.md`, stop, and inform the user.

### D. Checks or CodeRabbit are still `IN_PROGRESS`

Wait via a non-blocking scheduled check-in (§4).

## 4. Schedule the next non-blocking check-in

Never run a tight foreground `while` sleep-poll loop. Choose the check-in mechanism available in
your current Antigravity surface:

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

## Never

Sleep-poll in a foreground loop; resolve a thread without a disposition reply; resolve an unfixed
R11 security issue or a verified unfixed correctness defect; ask a person to resolve a review
thread; skip, ignore, or weaken a test to get CI green; push directly to `main`.
