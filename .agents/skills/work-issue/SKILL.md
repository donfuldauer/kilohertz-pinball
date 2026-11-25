---
name: work-issue
description: "Takes one Kilohertz Pinball GitHub issue from start to merge: checks its dependencies are merged, claims it, branches, routes the work through the work-classifier and agents, verifies, opens a PR that closes the issue, runs the CodeRabbit gate review when the issue completes a phase or milestone, merges, and unblocks dependent issues. Use when the user says 'Start work on issue X', 'work on #X', 'pick up issue X', 'start work on the next ready issue', 'Work on the next issue', or similar. Merging an issue/ branch starts a fresh session for the next issue (§8) when a GitHub-triggered automation is configured."
---

# work-issue

The canonical instructions for this skill live in `.claude/skills/work-issue/SKILL.md` (shared by
Claude Code and Gemini / Antigravity). Read that file in full and follow it, using the Gemini /
Antigravity tool, subagent and model-tier mappings in `AGENTS.md` and `docs/PROCESS.md`.
