---
name: work-issue
description: "Takes one Kilohertz Pinball GitHub issue from start to merge: checks its dependencies are merged, claims it, branches, routes the work through the work-classifier and agents, verifies, opens a PR that closes the issue, drives the CodeRabbit review to resolution, merges, and unblocks dependent issues. Use when the user says 'Start work on issue X', 'work on #X', 'pick up issue X', 'start work on the next ready issue', 'Work on the next issue', or similar. Merging an issue/ branch starts a fresh session for the next issue (§8) when a GitHub-triggered automation is configured."
---

# work-issue

You are the **Orchestrator** (PROCESS.md P5) for exactly one GitHub issue. The issue is the unit
of work and its own spec; the plan is the set of `work-unit` issues (sub-issues of the tracking
issues #79 M1 and #80 M2), never a repository file. You route, spawn, commit, open the PR, drive
review, merge and log. You write no product code and review no code (R9).

Repository: `donfuldauer/kilohertz-pinball`. In Claude Code (and Gemini / Antigravity sessions with
a GitHub MCP server), use the GitHub MCP tools (`mcp__github__*`) for issues, comments, labels and
pull requests, and use the `gh` CLI for searches that filter by label:
`gh api -X GET search/issues -f q='repo:donfuldauer/kilohertz-pinball is:issue label:<label> …' -F per_page=100 -F page=<N>`,
paged explicitly until a page returns fewer than 100 items (never `--paginate`). In Gemini /
Antigravity sessions without GitHub MCP tools, use the official `gh` CLI (`gh issue`, `gh pr`,
`gh run`, `gh api`) for every GitHub step below, keeping explicit paging on `search/issues` when
`--paginate` is blocked (PROCESS.md P1 "Working with GitHub").

## 1. Load and check the issue

0. **No issue number** ("the next ready issue", "the next issue", "Work on the next issue"):
   - **Finish unblocking first.** A merge starts this session (§8), sometimes before the merging
     session reaches §7 step 3, and a session can end before finishing it. So run §7 step 3's
     sweep, with its exceptions, over every open `work-unit` issue labelled `blocked` (label
     search, explicit paging) before choosing. The comment is "Unblocked by #<N> (PR #<P>)"
     (PROCESS.md P6), naming the dependency whose merged PR satisfied it last (a replacement issue
     counts, §1 step 2). If no such issue and PR can be identified (for example the last open
     entry was a "None" condition), change the label without a comment and say so in the RUN_LOG
     row.
   - **Resume abandoned work next.** Candidates are open issues labelled `in-progress` (a closed
     issue that still carries the label is skipped) and your open PRs. Neither may belong to an
     issue labelled `status:blocked-failure` or `blocked`; those wait for a person or for their
     dependencies. One is abandoned only if it has no governing claim (step 4): no unreleased
     claim comment on it names a live session. Never resume work a live session owns.
   - **Otherwise** list ready issues as `run-plan` §1 does and take the lowest `KP-NNN` whose
     owned paths overlap no active issue.
   - Claim it with step 4. If the claim loses, pick the next candidate.
   - If none qualifies, tell the user why and stop.
1. Read the issue (`issue_read`, method `get`, or `gh issue view`): title, body, labels, state,
   sub-issue parent.
   Stop if it is closed, if it is not a work unit (no `Plan ID` line; tracking issues are not work
   units), or if it is labelled `status:blocked-failure`: a person must clear that label before it
   is claimed. If it is labelled `blocked` and its "Depends on" section has no entries at all, a
   person set that hold: stop and say so (§7 step 3). Otherwise run the dependency check of step
   2: if every dependency is satisfied the label is stale, so replace it with `ready`
   (`issue_write`, the whole label list, or `gh issue edit`) and comment as step 0 says ("Unblocked
   by #<N> (PR #<P>)", or no comment when none can be identified); otherwise stop as step 2 says.
   Without an issue number, exclude this candidate instead and continue with the next one under
   step 0.
2. **Dependencies.** The body's "Depends on" section lists `#numbers`; it is the only dependency
   record. Every one must be **satisfied** under the merged-PR rule (PROCESS.md P6 step 5): closed
   as completed, and closed by a merged PR (via MCP: read the blocker with `issue_read`, method
   `get` — `closed_by_pull_requests.references` lists the PRs that closed it — and confirm each
   candidate with `pull_request_read`, method `get`, `merged` true; via `gh` CLI: run
   `gh issue view <n> --json state,stateReason,closedByPullRequestsReferences` — its
   `closedByPullRequestsReferences` array lists the closing PRs — and confirm each with
   `gh pr view <p> --json mergedAt` (`mergedAt` non-null). Fallback for an unlinked PR body that
   uses `Closes #<n>` or `Fixes #<n>`: run
   `gh api -X GET search/issues -f q='repo:donfuldauer/kilohertz-pinball is:pr is:merged base:main in:body "Closes #<n>"' -F per_page=100 -F page=<P>`,
   paged explicitly until a page returns fewer than 100 items, filter to PRs whose body contains a
   whole-number match for `Closes #<n>` (`#<n>` not followed by another digit, e.g. `#12` must not
   match `#123`), and only if no matching PR remains after paging and whole-number filtering run the
   same explicitly paged and whole-number-filtered query for `"Fixes #<n>"`). A blocker closed
   without a merged PR counts
   only if a comment on it names the replacement issue(s) that delivered the work and those
   replacements pass the same test. A "None" entry is satisfied once any condition it states in
   words holds on `origin/main` (for example "once the process migration (docs/PROCESS.md) is on
   `main`": the file exists on `origin/main`). If any is unsatisfied, stop and tell the user which
   ones, and which of them are themselves ready to start. Do not start blocked work. Without an
   issue number, exclude this candidate instead and continue with the next one under step 0
   (abandoned candidates first, then ready issues).
3. Read the issues it depends on, `docs/DEVELOPMENT.md`, the SPEC sections and REQ IDs the issue
   names, and any `docs/DECISIONS.md` entries its area touches.
4. **Claim, then verify.** This applies whether or not the prompt named the issue. GitHub has
   no atomic lock, so the earliest claim wins, ordered by **comment ID**. Comment IDs are unique
   and increase over time, so there are no ties. A claim posted after yours always has a higher
   ID and can never supersede a claim that has already passed its check: that claim's session
   saw every lower-ID claim when it checked. Because the repository is public, ignore any
   claim-shaped comment from an untrusted account before applying lowest-ID and liveness rules:
   a claim comment is eligible to govern **only** when its GitHub REST `author_association` is
   `OWNER`, `MEMBER`, or `COLLABORATOR` (write/maintain/admin collaborator), or its `user.login`
   matches the authenticated harness account (`gh api user --jq .login`). An issue's **governing
   claim** is the lowest-ID eligible claim comment on it that is not released and names a live
   session: when `get_session` is available **and** resolves the claim's session ID, treat it as
   live if `get_session` reports `working` or `blocked` (and ended otherwise); when `get_session` is
   unavailable (for example in Gemini / Antigravity) **or** cannot resolve the claim's session ID
   (for example when Claude Code inspects a claim posted by a Gemini / Antigravity session), treat
   any unreleased claim from another session by a trusted author as live unless the owner confirms
   it is abandoned. A claim is **released** when its comment has been edited to start with
   `Released:` (MCP tools cannot delete comments, and editing keeps an audit trail). There is no
   time cutoff: label changes (including a person re-applying `ready`) never invalidate a live
   claim. A claim stops counting only when its session ends or releases it, and a session releases
   its own claim (`update_issue_comment`, body `Released: <reason>`, or
   `gh api -X PATCH repos/donfuldauer/kilohertz-pinball/issues/comments/<id> -f body='Released: <reason>'`)
   whenever it loses (step 4), marks the issue blocked (§4) or finishes (§7). Resuming abandoned
   work writes no separate record: claims from ended sessions are never live, so they cannot
   govern, and the lowest-ID live claim still wins among sessions resuming at the same time.
   Compare governing claims, never an issue's latest claim. This relies on GitHub comment IDs being
   allocated in creation order and listed as soon as they are created: observed behaviour that
   GitHub does not document. If it fails, two sessions can rarely start overlapping work; the P6
   merge conditions often, but not always, surface that as a conflict or a red check.
   1. Post the claim comment (`add_issue_comment` or `gh issue comment`): work has started, with
      the branch name and this session's ID (`get_session` with no argument in Claude Code, or the
      conversation/session ID and UTC timestamp in Gemini / Antigravity). Keep the returned comment
      ID.
   2. Re-read **all** the issue's comments (`issue_read`, method `get_comments`, every page, or
      `gh api -X GET repos/donfuldauer/kilohertz-pinball/issues/<N>/comments -F per_page=100 -F page=<P>`
      paged explicitly with `P = 1, 2, …` until a page returns fewer than 100 items, never relying
      on the default 30-item page), its labels and its state. Filter out claim-shaped comments from
      untrusted authors (requiring `author_association` in `OWNER`, `MEMBER`, `COLLABORATOR`, or
      `user.login` matching `gh api user --jq .login`), and compare only eligible claim IDs after
      the full comment set has been fetched. Your claim loses, and you release it, if the issue is
      now closed or labelled `status:blocked-failure` or `blocked`, if the governing claim is not
      yours, or (without an issue number) if the issue no longer qualifies as the candidate step 0
      chose it as: a ready candidate must still carry `ready` and must not have gained
      `in-progress` from another session; an abandoned candidate must still have no other governing
      claim. With an issue number, tell the user which session owns the issue and stop. Without
      one, return to step 0 and pick the next candidate.
   3. **Path overlap.** A live claim reserves the issue's owned paths from the moment it is
      posted until the issue merges or is marked blocked. After posting yours, check this
      issue's owned paths against every other reservation: issues labelled `in-progress`, issues
      with an open PR, and open issues that have a governing claim. Issues labelled
      `status:blocked-failure` or `blocked` reserve nothing. If one overlaps, compare your
      claim's ID with that issue's governing claim's ID: the lower ID wins (an `in-progress`
      issue or open PR with no governing claim wins outright, unless your issue also has no
      other governing claim because you are resuming it as abandoned work: then the issue with
      the lower `KP-NNN` plan ID wins, so two abandoned overlapping issues never block each
      other). **Re-verify a loss once.** Two sessions can each lose at the same moment: one to
      the plan-ID tie-break against an abandoned issue, the other to the first one's claim
      before it is released. So before releasing a losing claim, wait about 30 seconds, then
      repeat with fresh reads the dependency check of §1 step 2 (every dependency still
      satisfied), the checks of step 4.2 (issue still open and not blocked, governing claim
      still yours and, without an issue number, the candidate still qualifies) and this overlap
      check. If all of them pass and you now win against every live overlapping claim,
      continue. Otherwise release your claim; if the dependency check failed, act as in §1
      step 2 (report the unsatisfied dependencies and which of them are ready, or, without an
      issue number, move on to the next candidate), and for any other failure act as in step
      4.2. This cannot let two overlapping sessions both proceed: a session only proceeds after
      a check in which its claim beats every live overlapping claim, and the lower comment ID
      always beats the higher.
   4. Only after the claim holds, run the §1 step 2 dependency check once more with fresh reads.
      If a dependency is no longer satisfied, release your claim and act as in §1 step 2.
      Otherwise add the `in-progress` label and remove `ready` (`issue_write` with the full new
      label list), then re-read the issue once more: if it is now closed or labelled
      `status:blocked-failure` or `blocked`, remove `in-progress`, release your claim and act as
      in step 4.2. Checking after the label change closes the window between the last check and
      the change; a block or close after this point is handled like any other blocked or closed
      issue. The cleanup leaves the issue with neither `ready` nor `in-progress`. The person who
      clears `status:blocked-failure` replaces it with `ready` (as §7 step 3 does for
      dependencies), and the person who reopens a closed issue for more work adds `ready` when
      reopening it; either makes the issue selectable again.

## 2. Branch

Each issue gets its own git worktree, so issues running in parallel never share a working tree
(if a sandbox restricts sibling directories and only one issue runs in the VM, use the
single-checkout fallback below). One branch, one PR, one issue.

0. **Select `<branch>`.** Normally `<branch>` is `issue/<N>-<short-slug>`: when GitHub-triggered
   session chaining is enabled, the next session starts only when an `issue/` branch merges (§8).
   If the session's environment can only push to an assigned branch, set `<branch>` to that
   assigned branch, retain its existing local/remote history (never reset it to `origin/main`),
   and note in the PR body that the chain will not continue from this merge: the owner restarts it
   with the scheduled task's "Run now".
1. `git fetch --prune origin`. Look for an existing branch and worktree for the issue:
   - for normal `issue/<N>-<short-slug>` branches, run `git branch -a --list "*issue/<N>-*"` and
     `git worktree list`;
   - for an assigned `<branch>`, check exact local and remote refs with
     `git show-ref --verify --quiet "refs/heads/<branch>"`,
     `git show-ref --verify --quiet "refs/remotes/origin/<branch>"`, and `git worktree list` (so a
     prefix-matching branch such as `<branch>-old` does not match `<branch>`).
2. **Found (a restart, or an existing assigned `<branch>`).** If step 1 discovered an existing
   `issue/<N>-*` branch, set `<branch>` to that discovered branch name before running the commands
   below. Resume it; never reset it to `origin/main`.
   - *Worktree:* reuse the worktree if it exists, otherwise
     `git worktree add ../kilohertz-issue-<N> <branch>` (for a remote-only branch,
     `git worktree add -b <branch> ../kilohertz-issue-<N> origin/<branch>`). In the worktree, if the
     remote branch exists, `git merge --ff-only origin/<branch>`.
   - *Single-checkout fallback:* if the local branch exists, `git checkout <branch>`; for a
     remote-only branch, `git checkout -b <branch> origin/<branch>`. If the remote branch exists,
     `git merge --ff-only origin/<branch>`.
   Read the issue comments and the branch's commits, and continue from the step they show was
   reached.
3. **Not found.**
   - *Worktree:* `git worktree add -B <branch> ../kilohertz-issue-<N> origin/main`.
   - *Single-checkout fallback:* `git checkout -B <branch> origin/main`.
4. Every later step for this issue (agents' working directory, commits, pushes, checks) runs
   inside that worktree (or directly in the checkout for the single-checkout fallback). Cargo
   builds use `CARGO_TARGET_DIR=<repo-root>/target/<N>` (`docs/DEVELOPMENT.md` "Agent
   environment"). If a worktree was created, it is removed in §7.

## 3. Route and execute

1. Set `FABLE_ALLOWANCE` (Claude Code) or T3 availability (Gemini / Antigravity) (PROCESS.md P4):
   `insufficient` if the session reports a current rate limit, throttling or fallback at T3;
   otherwise `unknown`. In Claude Code, it becomes `sufficient` only through an owner `yes` in a
   Fable ask. On Gemini / Antigravity, it is `sufficient` when the active plan and quota support T3
   and no rate limit, throttling or fallback is reported.
2. Spawn `work-classifier` (via Claude Code's `Agent` tool or Antigravity's `invoke_subagent` /
   `define_subagent`, PROCESS.md P5) with: the full issue body, its number and URL, the full bodies
   of the issues it depends on (read via MCP `issue_read` or `gh issue view`; the classifier does
   not fetch issues itself), the tracking issue, the branch, and `FABLE_ALLOWANCE` / T3
   availability. For a `type:feature` or `type:fix` issue whose deliverables include code, ask for
   **two** briefs: Implementer and Independent Tester (owned paths disjoint). For `type:infra`,
   `type:docs` and `type:content`, ask for one Implementer brief whose TESTS REQUIRED include the
   issue's acceptance tests.
3. If the classifier returns `fable check: wanted, allowance unknown, owner ask` in a Claude Code
   session, run the Fable ask of PROCESS.md P4 (reminder first, one ask per batch, up to 30
   minutes, per-item answers; no answer means `unknown`, and `unknown` means no Fable). Meanwhile
   continue anything that does not depend on those items. On an owner `yes`, call the classifier
   again with `FABLE_ALLOWANCE: sufficient (owner, <UTC time>)`; otherwise use the T2 brief and put
   the `[Fable wanted]` note in this issue's RUN_LOG row. On Gemini / Antigravity, if T3 is
   unavailable or unconfirmed, use the T2 brief and record `[T3 wanted]` in the RUN_LOG row.
4. Spawn the agents the classifier names with the briefs verbatim and its model tier overrides
   (keeping `inherit` in Antigravity if the harness policy requires it); Implementer and
   Independent Tester in parallel.
5. `requests outside owned paths` or discovered work that this issue cannot absorb become a
   **new `work-unit` issue** (`issue_write`, method `create`, or `gh issue create`; body from
   `.github/ISSUE_TEMPLATE/work-unit.md` with the next free `KP-NNN`, "Depends on" filled in,
   labels like this one plus `blocked` or `ready`), attached to its tracking issue
   (`sub_issue_write`, method `add`, or `gh api` sub-issues endpoint, with the new issue's numeric
   `id`: #79 when M1 needs it, else #80), and linked from the PR. Never a queue line. No
   third-party pinball names in the issue (DECISIONS.md "Planning — No third-party names in
   planning text").
6. Commit the work (one commit per role or deliverable, subject prefixed `[#<N>]`), unless the
   brief said the executor commits.

## 4. Log and verify

First add the run-log row(s) for this issue to `docs/RUN_LOG.md` (phase = the issue's `phase:`
label, work item = `#<N>`, T3 / Fable notes in Notes) and commit, so the verified commit is the one
the PR opens on. Any later fix commit updates the row and is verified again.

Then spawn `work-verifier` with every brief and report, the branch and the base `origin/main`. On
`fail`: investigator if the cause is unclear, then classifier → Fixer → verify again, under the
P7 failure policy. Three failed attempts plus one tier-up attempt → mark the issue blocked
(label `status:blocked-failure`, remove `in-progress`, release this session's claim with
`Released: blocked-failure`), comment the failure history on the issue, add a
`docs/RUN_LOG.md` row, and stop.

## 5. Open the PR

1. Check the head is the commit the work-verifier last passed. Push:
   `git -c push.negotiate=true push -u origin <branch>`.
2. Determine the PR's **review mode label** (`docs/PROCESS.md` P6): carry the issue's
   `review:skip` / `review:sanity` / `review:full` label onto the PR, upgrading to `review:sanity`
   if the diff touches `crates/pin_core/`, `contracts/CHANGE_REQUESTS.md`, SPEC Appendix A/B
   schemas, concurrency/timing loops, `very-high` blast-radius paths, or R11 security boundaries
   (even if the issue lacked the label).
3. Create the PR (`create_pull_request` + label edit, or
   `gh pr create --base main --head <branch> --label <review:*>`). The body follows
   `.github/pull_request_template.md`, starts with `Closes #<N>`, states the review mode under
   `## Work item`, and fills every section (write "not applicable" with a reason rather than
   leaving one empty). Bot names go in backticks.
4. Watch the PR:
   - **Claude Code:** subscribe to its activity (`subscribe_pr_activity`). If the session cannot
     subscribe, schedule a check-in with `send_later` (about 10 minutes, or 3–5 minutes for a
     `review:skip` PR waiting only on CI) and end the turn with a one-line status; repeat check-ins
     until the PR merges.
   - **Gemini / Antigravity:** run the `pr-watch` skill (`.agents/skills/pr-watch/SKILL.md`), which
     schedules non-blocking check-ins (`/schedule` or a single async background check-in), handles
     CI failures and — on reviewed PRs (`review:sanity` / `review:full`, or whenever CodeRabbit
     actually posted a review) — responds to CodeRabbit findings, resolves review threads itself,
     and drives the PR to completion.
   Never sleep-poll in a foreground loop.

## 6. Review rounds

- **Unreviewed `review:skip` PRs** (initial-usable-build phase, while `.coderabbit.yaml` has
  `auto_review.enabled: false` and CodeRabbit has posted no review on the PR): do **not** request
  `@coderabbitai review`. Handle any CI failures through the investigator / classifier / Fixer /
  `work-verifier` loop, and proceed to §7 as soon as CI checks (`pr-checks` + any other PR
  workflows), `main` CI, and `work-verifier` are green.
- **Reviewed PRs (`review:sanity`, `review:full`, post-usable default, or any PR where CodeRabbit
  posted a review):** each CodeRabbit review round (or push) is one batch, handled with the
  `triage` skill's §2 procedure and PROCESS.md P6:
  - Read with `pull_request_read` (or `gh pr view` / `gh pr checks` / `gh api`): the reviews, the
    review comments and threads, the PR's issue comments (CodeRabbit's summary and pre-merge
    checks), and the head commit's status and check runs.
  - On **`review:sanity`** PRs, keep the pass targeted: fix blocking boundary, contract, schema,
    concurrency/timing, determinism (R3/R4), security (R11) and verified correctness defects;
    dispose of cosmetic style, naming or non-blocking refactor nitpicks as `Deferred` (linking
    `#153` KP-064 or a new M2 `work-unit` issue) or `Not a defect` rather than running multi-round
    polish loops.
  - Batch local fix commits before pushing so `auto_pause_after_reviewed_commits: 2` is not tripped
    unnecessarily.
  - Every finding gets exactly one disposition (Fixed / Not a defect / Deferred) recorded as a
    reply (`add_reply_to_pull_request_comment` or `gh api` review-comment reply), reason and
    evidence first.
  - Resolve with `resolve_review_thread`, GraphQL `resolveReviewThread` via `gh api graphql`, or
    the REST route
    `POST /repos/donfuldauer/kilohertz-pinball/pulls/<P>/ccr/comments/<comment_id>/resolve`. Never
    resolve an unfixed security issue (R11 boundary) or a verified unfixed correctness defect.
  - Failed pre-merge checks (for example the description check) are fixed by editing the PR body
    (`update_pull_request` or `gh pr edit`).
  - CI failures go through the same loop (investigator if the cause is unclear).
  - If CodeRabbit has not reviewed the head commit on a PR that requires review (for example it
    paused after `auto_pause_after_reviewed_commits: 2` or was rate-limited), post
    `@coderabbitai review` once (a PR comment, `add_issue_comment` or `gh pr comment`); at most two
    more asks, then the RUN_LOG row of P6 step 4.
  - When CodeRabbit's approval is all that is missing on a reviewed PR, post
    `@coderabbitai approve` (P6 step 3).

## 7. Merge

When every P6 step 5 condition holds on the head commit (`pr-checks` and every other PR check
green; if the PR requires or received CodeRabbit review, CodeRabbit reviewed the head with no
outstanding change request and every thread replied to and resolved; branch up to date with `main`
where P6 requires it; `main`'s last push CI green; last work-verifier result `pass` for the head):

1. Merge with a merge commit: `merge_pull_request` (method `merge`) or
   `gh pr merge <P> --merge --match-head-commit <sha>`, naming the head SHA.
2. Check the branch was deleted; delete it if not (`git push origin --delete <branch>`). If the
   session's proxy refuses, append a `[P8 setting]` line for it.
3. **Unblock dependents.** List the open issues labelled `work-unit` and `blocked` (label search,
   explicit paging) and re-check each one's "Depends on" list, whether or not it names `#<N>`
   (a dependency on a cancelled issue can be covered by a replacement issue). For each whose
   dependencies are now all satisfied (§1 step 2, replacements included), replace its `blocked`
   label with `ready` and comment "Unblocked by #<N> (PR #<P>)", naming the dependency whose merged
   PR satisfied it last (`#<N>` itself, a replacement, or another dependency); if none can be
   identified, change the label without a comment, as §1 step 0 says. The label change is one
   `issue_write` call with the full label list (`ready` in, `blocked` out) — or one `gh issue edit`
   adding `ready` and removing `blocked` — so the issue is never without both. Re-read the issue's
   labels (`issue_read`, method `get`, or `gh issue view`) immediately before that call and build
   the list from them; if `status:blocked-failure` is now present or `blocked` is gone, skip the
   write; if `ready` is already there, the write only removes `blocked`.
   Exceptions:
   - skip an issue that no longer carries `blocked`: the next session may already have unblocked
     it (§1 step 0);
   - skip one that also carries `status:blocked-failure`: a person must clear that hold
     (PROCESS.md P7);
   - leave `blocked` on an issue whose "Depends on" section has no entries at all: a person set
     that hold. Conditional entries (a "None" entry that states a condition in words) are
     evaluated with §1 step 2's check and unblock the issue once they hold;
   - do not post an "Unblocked" comment if one is already there.
4. Remove `in-progress` from the issue if it is still present (it closes through `Closes #<N>`),
   and release this session's claim (`Released: merged in PR #<P>`).
5. Remove the issue's worktree from the main checkout (if a worktree was used):
   `git worktree remove ../kilohertz-issue-<N>`.
6. Tell the user: what merged, the PR link, the issues it unblocked, and the next ready issues
   in plan order. The merge commit goes in the RUN_LOG row of the next PR.
7. Do nothing to start the next session (§8).

## 8. Next session

In **Claude Code**, a scheduled task with a GitHub trigger on pull requests merged into `main` from
`issue/` branches starts a fresh session with the prompt "Work on the next issue". This session
does nothing to hand off: never call `create_session`, `fire_trigger` or `update_trigger` to chain.
A session started by another session asks a person to approve every remote-session tool call, and a
session can neither fire nor edit a task it did not create. The chain runs until no issue is ready,
or until a session ends without merging an `issue/` branch; the owner then restarts it with the
task's "Run now". In **Gemini / Antigravity**, there is no native GitHub-merge webhook trigger;
start the next session manually (or use a persistent polling `/automation` job or `/run-plan`). If
no scheduled task exists yet, nothing starts automatically: see the P8 line in
`docs/HUMAN_QUEUE.md` and DECISIONS.md "Process — Next session starts from a merged issue/ PR".

## Never

Start a session to chain to the next issue; start an issue whose dependencies are not
satisfied; push to `main`; merge with a failing required check or an unresolved thread; skip,
ignore or weaken a test; leave a judgement call in `docs/HUMAN_QUEUE.md` instead of
`docs/DECISIONS.md`; ask a person to resolve a review thread.
