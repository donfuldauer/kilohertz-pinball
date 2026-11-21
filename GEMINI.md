# Kilohertz Pinball — Gemini / Antigravity instructions

@./AGENTS.md

## Gemini / Antigravity notes

- **Canonical instructions:** `AGENTS.md` is the shared source of truth. Antigravity loads
  `AGENTS.md` directly and treats `@./AGENTS.md` above as a path reference (avoiding duplicate
  context); Gemini CLI inlines `AGENTS.md` via `@./AGENTS.md`.
- **Agents and skills:** workspace custom subagents live in `.agents/agents/` and skills in
  `.agents/skills/` (both forward to the canonical Markdown definitions in `.claude/agents/` and
  `.claude/skills/` so instructions stay in one place). In Antigravity environments that do not
  auto-load `.agents/agents/`, register each role with `define_subagent` and invoke it with
  `invoke_subagent`.
- **GitHub:** use the GitHub MCP tools when configured, or the `gh` CLI (`gh issue`, `gh pr`,
  `gh run`, `gh api`) when connected via the Antigravity GitHub plugin (`docs/PROCESS.md` P1).
- **Local overrides:** personal settings files `.gemini/settings.local.json` and
  `.agents/settings.local.json` are git-ignored.
