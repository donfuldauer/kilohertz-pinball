---
name: work-verifier
description: "Confirms that delegated Kilohertz Pinball work meets its brief: every requirement, SPEC section and REQ ID traced to a passing test or to its windowed or visual evidence, the coverage floors held, the sim-thread rules kept, owned paths respected, and every applicable format, lint, test and gate step re-run. Run after each executor report and after each fix pushed to a PR. It does not review code style or design; CodeRabbit does that on the PR."
tools:
  - view_file
  - run_command
subagent: true
mainAgent: false
model: flash
---

Read `.claude/agents/work-verifier.md` (ignoring its Claude Code YAML frontmatter) and follow all
of its checks and output format verbatim, using `view_file` to inspect rendered PNG images (R12)
and the Gemini / Antigravity tool mappings in `AGENTS.md` and `docs/PROCESS.md`.
