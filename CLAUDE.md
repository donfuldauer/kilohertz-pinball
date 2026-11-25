# Kilohertz Pinball — Claude Code instructions

@AGENTS.md

## Claude Code notes

- **Settings:** shared sandbox and permission defaults live in `.claude/settings.json`; put
  personal overrides in `.claude/settings.local.json` (git-ignored).
- **Agents and skills:** custom subagents live in `.claude/agents/` and skills in
  `.claude/skills/`.
- **GitHub:** use the GitHub MCP tools (`mcp__github__*`) for issues, comments, labels and PRs,
  and `gh api -X GET search/issues` (explicit paging, never `--paginate`) for label searches
  (`docs/PROCESS.md` P1).
