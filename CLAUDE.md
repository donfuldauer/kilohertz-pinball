# Kilohertz Pinball — agent instructions

Kilohertz Pinball is a cross-platform pinball simulation engine in Rust with a 1 kHz fixed-step
simulation. What it must be is `docs/SPEC.md`, the normative specification (it was
`docs/PLAN.md`; its section, subphase and D1–D33 numbers are unchanged, so "PLAN §3.2" in code
means SPEC §3.2). How work is done is `docs/PROCESS.md` (cite it as P1–P8, rules R1–R12). Layout,
commands, the agent environment, CI, gates and contract owners are in `docs/DEVELOPMENT.md`;
delegation briefs follow `docs/BRIEF_TEMPLATE.md`. The plan is the set of GitHub issues labelled
`work-unit`, grouped as sub-issues of the tracking issues #79 (M1 — Play experience) and #80
(M2 — Backlog); it is never written into the repository.

**Starting work:** each issue is one unit of work with its dependencies. When the user says
"Start work on issue X", use the `work-issue` skill: it checks the issue's dependencies are
merged, drives the work through the agents below, opens a PR that closes the issue, resolves the
CodeRabbit review and merges. "Start work on the next ready issue" (or "Work on the next
issue") picks one, and merging an `issue/` branch starts a fresh session for the next issue
through a scheduled task (work-issue §8). The `run-plan` skill loops over every
ready issue in plan order (`KP-NNN`). The `plan-from-spec` skill turns SPEC requirements into
issues.

- **Model routing (P4).** Every unit of work goes through the `work-classifier` agent first,
  except items that pass the `triage` skill's fast-path checks; each review round or push is one
  batch (P5). Tiers: Haiku 5.5, Sonnet 5.5, Opus 5.5. **Fable 5.2 only** for very-high-blast-radius
  (P3) or extremely hard-to-troubleshoot work, with evidence, **and** only when the account's
  current usage limits leave enough Fable allowance. The agent cannot read usage itself, so when
  Fable is wanted you ask the owner and wait up to 30 minutes (P4 "Fable ask"); no answer means
  unknown, and unknown means no Fable. A `[Fable wanted]` note goes in the unit's RUN_LOG row.
- **Agents (P5).** `.claude/agents/`: `work-classifier`, `task-executor`, `work-verifier`,
  `issue-investigator`. Pass the classifier's brief verbatim and its model as the `model`
  override. Subagents cannot spawn subagents; you do every spawn.
- **No self-review (R9).** Do not review code yourself or with a sibling agent. Before the PR,
  run the CodeRabbit CLI on the branch once (at most twice) and fix its findings (P6 step 1;
  skipped if the CLI is unavailable). Open a PR against
  `main` (body from `.github/pull_request_template.md`, starting `Closes #<N>`), subscribe to its
  activity, give every CodeRabbit finding a disposition reply, resolve every thread yourself when
  justified (never leave one for a person; never resolve an unfixed security issue or a verified
  unfixed correctness defect), and merge without waiting for a person once the P6 merge
  conditions hold (`pr-checks` green, `main` green, verifier `pass`).
- **GitHub (P1).** Use the GitHub MCP tools for issues, comments, labels and PRs; use
  `gh api -X GET search/issues` (explicit paging) only for searches by label. Dependencies are
  the issue body's "Depends on" list; a claim is released by editing it to `Released: <reason>`.
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
