#!/usr/bin/env bash
# Enables the tracked hooks in .githooks/ for this clone and records which human the
# commit-msg hook should credit as co-author on agent-made commits. Idempotent and quiet.
#
# Agent harnesses run this automatically (Claude Code: the SessionStart hook in
# .claude/settings.json; Antigravity cloud VMs: .agents/setup-env.sh). Contributors can also run
# it by hand once per clone.
#
# The co-author ("Name <email>") is stored in this clone's `git config kilohertz.coauthor` and
# is resolved, first match wins, from:
#   1. $KILOHERTZ_COAUTHOR                  e.g. KILOHERTZ_COAUTHOR="Jane Doe <jane@example.com>"
#   2. an existing `git config kilohertz.coauthor` (set it in ~/.gitconfig to apply to every clone)
#   3. git's user.name/user.email, when that identity is a person rather than an agent
#   4. the GitHub account `gh` is authenticated as, using its no-reply address
#      (<id>+<login>@users.noreply.github.com), which is how cloud agent sessions resolve it
# If none resolves, the hook prints a warning on each commit and adds no trailer.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${REPO_ROOT}"

if [[ "$(git config --get core.hooksPath || true)" != ".githooks" ]]; then
  git config core.hooksPath .githooks
  echo "git hooks: core.hooksPath set to .githooks"
fi

# shellcheck source=../.githooks/lib-agent-identity.sh
source .githooks/lib-agent-identity.sh

coauthor="${KILOHERTZ_COAUTHOR:-}"
[[ -n "${coauthor}" ]] || coauthor="$(git config --get kilohertz.coauthor || true)"

if [[ -z "${coauthor}" ]]; then
  name="$(git config --get user.name || true)"
  email="$(git config --get user.email || true)"
  if [[ -n "${name}" && -n "${email}" ]] && ! is_agent_identity "${name}" "${email}"; then
    coauthor="${name} <${email}>"
  fi
fi

if [[ -z "${coauthor}" ]] && command -v gh >/dev/null 2>&1; then
  gh_user="$(gh api user --jq '"\(.id) \(.login) \(.name // "")"' 2>/dev/null || true)"
  if [[ -n "${gh_user}" ]]; then
    read -r gh_id gh_login gh_name <<<"${gh_user}"
    coauthor="${gh_name:-${gh_login}} <${gh_id}+${gh_login}@users.noreply.github.com>"
  fi
fi

if [[ -n "${coauthor}" ]]; then
  if [[ "$(git config --local --get kilohertz.coauthor || true)" != "${coauthor}" ]]; then
    git config --local kilohertz.coauthor "${coauthor}"
    echo "git hooks: agent commits in this clone will be co-authored by ${coauthor}"
  fi
else
  echo "git hooks: could not determine a human co-author for agent commits;" \
       "set one with: git config kilohertz.coauthor 'Name <email>'" >&2
fi
