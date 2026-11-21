---
name: run-plan
description: "The Orchestrator's loop over the Kilohertz Pinball plan's GitHub issues: finds every ready work-unit issue (all dependencies merged), works each one with the work-issue skill (in parallel where owned paths do not overlap), drives PRs to merge, runs phase gates, and keeps the run log current. Use to start or resume unattended work across many issues (/run-plan, optionally with a tracking issue or phase)."
---

# run-plan

The canonical instructions for this skill live in `.claude/skills/run-plan/SKILL.md` (shared by
Claude Code and Gemini / Antigravity). Read that file in full and follow it, using the Gemini /
Antigravity tool, subagent and model-tier mappings in `AGENTS.md` and `docs/PROCESS.md`.
