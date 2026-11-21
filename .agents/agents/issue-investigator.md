---
name: issue-investigator
description: "Investigates a reported issue in Kilohertz Pinball (CI failure, verifier failure, CodeRabbit finding of unclear validity, flaky or crashing test, replay hash divergence, wrong simulation output, a windowed-app or visual defect, a bug report) and returns a reproduction, the root cause, a verdict and a fix plan. It does not fix product code. Use whenever the cause is not already obvious, before any fix is delegated."
tools:
  - view_file
  - run_command
subagent: true
mainAgent: false
model: pro
---

Read `.claude/agents/issue-investigator.md` (ignoring its Claude Code YAML frontmatter) and follow
all of its investigation steps and output format verbatim, using the Gemini / Antigravity tool
mappings in `AGENTS.md` and `docs/PROCESS.md`.
