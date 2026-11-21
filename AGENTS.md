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
drives the work through the agents below, opens a PR that closes the issue, resolves the CodeRabbit
review and merges. "Start work on the next ready issue" (or "Work on the next issue") picks one;
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
    leave enough Fable allowance. Because the agent cannot read `/usage` itself, when Fable is
    wanted ask the owner and wait up to 30 minutes (P4 "Fable ask"); no answer means unknown, and
    unknown means no Fable (log `[Fable wanted]` in `docs/RUN_LOG.md`).
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
- **No self-review (R9).** Do not review code yourself or with a sibling agent. Open a PR against
  `main` (body from `.github/pull_request_template.md`, starting `Closes #<N>`), watch its activity
  (Claude Code: `subscribe_pr_activity` or `send_later`; Gemini / Antigravity: run the `pr-watch`
  skill in `.agents/skills/pr-watch/SKILL.md` to schedule non-blocking check-ins and drive
  CodeRabbit to approval; never sleep-poll in a foreground loop), give every CodeRabbit finding a
  disposition reply, resolve every thread yourself when justified (never leave one for a person;
  never resolve an unfixed security issue or a verified unfixed correctness defect), and merge
  without waiting for a person once the P6 merge conditions hold (`pr-checks` green, `main` green,
  verifier `pass`).
- **GitHub (P1).** In Claude Code (and Gemini/Antigravity sessions with a GitHub MCP server), use
  the GitHub MCP tools (`mcp__github__*`) for issues, comments, labels and PRs, and
  `gh api -X GET search/issues` (explicit paging) for searches by label. In Gemini / Antigravity
  sessions without GitHub MCP tools, use the official `gh` CLI (`gh issue`, `gh pr`, `gh run`,
  `gh api`, with explicit paging on `search/issues` when a proxy blocks `--paginate`). Dependencies
  are the issue body's "Depends on" list; a claim is released by editing it to `Released: <reason>`.
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
