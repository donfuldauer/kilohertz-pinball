# Kilohertz Pinball — agent instructions

Kilohertz Pinball is a cross-platform pinball simulation engine in Rust with a 1 kHz fixed-step
simulation. These instructions apply to both **Claude Code** (`CLAUDE.md`, `.claude/`) and
**Gemini / Antigravity** (`GEMINI.md`, `.agents/`). What the engine must be is `docs/SPEC.md`, the
normative specification (it was `docs/PLAN.md`; its section, subphase and D1–D33 numbers are
unchanged, so "PLAN §3.2" in code means SPEC §3.2). How work is done is `docs/PROCESS.md` (cite it
as P1–P8, rules R1–R12). Layout, commands, the agent environment, CI, gates and contract owners are
in `docs/DEVELOPMENT.md`; delegation briefs follow `docs/BRIEF_TEMPLATE.md`. The plan is the set of
GitHub issues labelled `work-unit`, grouped as sub-issues of the tracking issues #79 (M1 — Play
experience) and #80 (M2 — Backlog); it is never written into the repository.

**Starting work:** each issue is one unit of work with its dependencies. When the user says
"Start work on issue X", use the `work-issue` skill (`.claude/skills/work-issue/SKILL.md`,
forwarded by `.agents/skills/work-issue/SKILL.md`): it checks the issue's dependencies are merged,
drives the work through the agents below, opens a PR that closes the issue, runs the CodeRabbit
gate review when the issue completes a phase or milestone, and merges. "Start work on the next ready issue" (or "Work on the next issue") picks one;
in Claude Code, a GitHub-triggered scheduled task can start a fresh session when an `issue/` branch
merges (`work-issue` §8), whereas in Gemini / Antigravity the next session is started manually or
by a polling `/automation` job. The `run-plan` skill loops over every ready issue in plan order
(`KP-NNN`). The `plan-from-spec` skill turns SPEC requirements into issues.

- **Model routing (P4).** Every unit of work goes through the `work-classifier` agent first,
  except items that pass the `triage` skill's fast-path checks; each review round or push is one
  batch (P5). Tiers (details in `docs/PROCESS.md` P4):
  - **Claude Code:** T0 Haiku 5.5 (`haiku`), T1 Sonnet 5.5 (`sonnet`), T2 Opus 5.5 (`opus`), and
    **T3 Fable 5.2 (`fable`) only** for very-high-blast-radius (P3) or extremely
    hard-to-troubleshoot work, with evidence, **and** only when the account's current usage limits
    leave enough Fable allowance. An allowance the agent cannot read counts as not enough: it
    never asks the owner or waits, but runs at T2 and logs `[Fable wanted]` in `docs/RUN_LOG.md`.
  - **Gemini / Antigravity:** T0 Flash-Lite (`flash_lite`), T1 Flash (`flash`), T2 Pro (`pro`, or
    `inherit` when the session already runs on a Pro/reasoning model), and **T3 escalation**
    (`pro` at maximum reasoning) only under the same P4 need and T2-exhaustion rules when plan and
    quota allow (otherwise run at T2 and log `[T3 wanted]` in `docs/RUN_LOG.md`). Keep `inherit`
    on Antigravity subagent calls unless a tier override is requested or needed by the harness
    policy.
- **Agents (P5).** Four subagent roles: `work-classifier`, `task-executor`, `work-verifier`,
  `issue-investigator`. Canonical role prompts live in `.claude/agents/` (loaded directly by Claude
  Code); Antigravity custom subagent definitions live in `.agents/agents/` and forward to them. In
  Antigravity environments that do not auto-load `.agents/agents/`, register each role with
  `define_subagent` and run it with `invoke_subagent`. (Standalone Gemini CLI does not load
  `.agents/agents/`; see `GEMINI.md`.) Pass the classifier's brief verbatim and its model tier
  override. Subagents never spawn subagents; the Orchestrator (main session) does every spawn.
- **No self-review (R9) and review rhythm (P6).** Do not review code yourself or with a sibling
  agent. CodeRabbit reviews **only at the completion of a phase or milestone**, never on every
  unit of work. Routine PRs (against `main`, body from `.github/pull_request_template.md`,
  starting `Closes #<N>`, no `review:*` label) merge on `pr-checks`, green `main` CI and
  `work-verifier` (`pass`) with no CodeRabbit review. When an issue is the last open `work-unit`
  issue of its `phase:*` label or of its tracking issue (or is labelled `review:gate`, such as
  `#153` / KP-064 after Gate A3), it is a gate issue: open a review-only gate review PR (gate
  branch → `review-base/<gate>`, cumulative since the last reviewed commit in `docs/RUN_LOG.md`),
  request `@coderabbitai full review`, give every finding a disposition reply, resolve every
  thread yourself when justified (never leave one for a person; never resolve an unfixed security
  issue or a verified unfixed correctness defect), and merge the gate issue's PR once CodeRabbit
  approves and the P6 gate merge conditions hold; then close the review PR unmerged. Watch every
  PR (Claude Code: `subscribe_pr_activity` or `send_later`; Gemini / Antigravity: `pr-watch` in
  `.agents/skills/pr-watch/SKILL.md`; never sleep-poll) and merge without waiting for a person.
- **GitHub (P1).** In Claude Code (and Gemini/Antigravity sessions with a GitHub MCP server), use
  the GitHub MCP tools (`mcp__github__*`) for issues, comments, labels and PRs, and
  `gh api -X GET search/issues` (explicit paging) for searches by label. In Gemini / Antigravity
  sessions without GitHub MCP tools, use the official `gh` CLI (`gh issue`, `gh pr`, `gh run`,
  `gh api`, with explicit paging on `search/issues` when a proxy blocks `--paginate`). Dependencies
  are the issue body's "Depends on" list; a claim is released by editing it to `Released: <reason>`.
- **Commit attribution (all harnesses).** Agent-made commits credit the human contributor the
  agent works for as well as the agent. `scripts/install-git-hooks.sh` (run by the Claude Code
  `SessionStart` hook in `.claude/settings.json` and by `.agents/setup-env.sh`; otherwise run it
  once per clone) enables `.githooks/commit-msg` and records the contributor in
  `git config kilohertz.coauthor` (from `$KILOHERTZ_COAUTHOR`, an existing `kilohertz.coauthor`, a
  non-agent `user.name`/`user.email`, or the GitHub account `gh` is signed in to, via its
  no-reply address). The hook appends `Co-authored-by: <contributor>` to every commit authored by an
  agent identity (`noreply@anthropic.com`, `[bot]` accounts). Keep the agent as author; never
  rewrite authorship to hide it, and never bypass the hook with `--no-verify`.
- **Non-negotiables:** determinism (R3); no allocation, mutex or file I/O on the sim thread (R4);
  untrusted tables stay untrusted (R11); evidence for what a player sees and touches: the D30
  image-test order and the real windowed app, never headless only (R12). `pin_core` contracts
  change only through `contracts/CHANGE_REQUESTS.md` (R2).
- Record ambiguous choices and judgement calls in `docs/DECISIONS.md` (your recommendation is the
  decision) and continue (R8). `docs/HUMAN_QUEUE.md` is only for P8 items a person must do
  (`- [P8 <kind>] [<id>] <text>`).
- No third-party pinball names (manufacturers, machines, games, products, websites) in planning
  text: SPEC, issues, DECISIONS.md entries, briefs (DECISIONS.md "Planning — No third-party names
  in planning text"). Technical dependencies (SDL3, wgpu, mlua, wasmtime, parry3d, DOF, X-Arcade)
  are integrations and may be named.
