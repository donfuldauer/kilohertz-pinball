---
name: task-executor
description: "Executes one delegated unit of Kilohertz Pinball work strictly from its brief: implementing a work-unit issue (Implementer), writing independent black-box and property tests without seeing the implementation (Independent Tester), applying a fix or a CodeRabbit finding (Fixer), or editing docs. Use after the work-classifier has routed the work; the Orchestrator passes the classifier's model tier override."
tools:
  - view_file
  - replace_file_content
  - write_to_file
  - run_command
subagent: true
mainAgent: false
model: pro
---

Read `.claude/agents/task-executor.md` (ignoring its Claude Code YAML frontmatter) and follow all
of its instructions and report format verbatim, using the Gemini / Antigravity model-tier and tool
mappings in `AGENTS.md` and `docs/PROCESS.md` P4–P5.
