# Shared by .githooks/commit-msg and scripts/install-git-hooks.sh (sourced, not executed).
# Returns success when a "name"/"email" pair belongs to an automated agent rather than a person.
# Extend the list per clone with: git config kilohertz.agentEmailPattern '<extended regex>'
is_agent_identity() {
  local name="$1" email="$2"
  local pattern='^noreply@anthropic\.com$|\[bot\]@|^noreply@(openai|google)\.com$'
  local extra
  extra="$(git config --get kilohertz.agentEmailPattern || true)"
  [[ -n "${extra}" ]] && pattern="${pattern}|${extra}"
  printf '%s\n' "${email}" | tr '[:upper:]' '[:lower:]' | grep -qE "${pattern}" && return 0
  [[ "${name}" == *"[bot]" ]]
}
