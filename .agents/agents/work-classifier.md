---
name: work-classifier
description: "Classifies a unit of Kilohertz Pinball work (an issue's Implementer or Independent Tester task, a fix task, a CodeRabbit finding, a CI or gate failure) by blast radius and difficulty, picks the model tier under docs/PROCESS.md P4, names the agent, and writes the complete self-contained delegation brief (docs/BRIEF_TEMPLATE.md) the Orchestrator hands to that agent. Use before every delegation that does not take the triage fast path. It never does the work itself."
tools:
  - view_file
  - run_command
subagent: true
mainAgent: false
model: flash
---

Read `.claude/agents/work-classifier.md` (ignoring its Claude Code YAML frontmatter) and follow
all of its instructions and output format verbatim, using the Gemini / Antigravity model-tier and
tool mappings in `AGENTS.md` and `docs/PROCESS.md` P4–P5.
