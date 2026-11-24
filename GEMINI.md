# Kilohertz Pinball — Gemini / Antigravity instructions

@./AGENTS.md

## Gemini / Antigravity notes

- **Canonical instructions:** `AGENTS.md` is the shared source of truth. Antigravity loads
  `AGENTS.md` directly and treats `@./AGENTS.md` above as a path reference (avoiding duplicate
  context); Gemini CLI inlines `AGENTS.md` via `@./AGENTS.md`.
- **Skills:** workspace skills live in `.agents/skills/` (discovered by both Antigravity and
  Gemini CLI): `work-issue`, `run-plan`, `plan-from-spec` and `triage` forward to the canonical
  Markdown definitions in `.claude/skills/`, and `pr-watch` (`.agents/skills/pr-watch/SKILL.md`)
  is the Gemini / Antigravity-specific PR check-in loop (CI on every PR, CodeRabbit findings on
  gate review PRs).
- **Subagents (Antigravity vs. Gemini CLI):**
  - **Antigravity:** workspace custom subagents live in `.agents/agents/` (forwarding to the
    canonical role prompts in `.claude/agents/`). In Antigravity environments that do not
    auto-load `.agents/agents/`, register each role with `define_subagent` and invoke it with
    `invoke_subagent`.
  - **Gemini CLI:** Gemini CLI does not discover `.agents/agents/` and does not have Antigravity's
    `define_subagent` / `invoke_subagent` tools. For full multi-agent P4/P5 delegation, use
    Antigravity (App, IDE, or `agy` CLI) or Claude Code. In a standalone Gemini CLI session, the
    main session does not run the executor or verifier roles itself; use the canonical role prompts
    in `.claude/agents/` only through custom subagents mirrored into local `.gemini/agents/`.
- **GitHub:** use the GitHub MCP tools when configured, or the `gh` CLI (`gh issue`, `gh pr`,
  `gh run`, `gh api`) when connected via the Antigravity GitHub plugin (`docs/PROCESS.md` P1).
- **Settings:** Antigravity stores personal configuration under `~/.gemini/config/`. Gemini CLI
  uses `~/.gemini/settings.json` for user settings and `.gemini/settings.json` for project
  settings (`.gemini/settings.json` is git-ignored so personal project overrides stay out of Git).
