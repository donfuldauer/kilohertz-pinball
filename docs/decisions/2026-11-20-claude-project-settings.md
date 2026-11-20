# Decisions: claude-project-settings (checked-in Claude Code settings)

## claude-project-settings — Claude project settings

**Decision:** Ship `.claude/settings.json` with the sandbox on and `autoAllowBashIfSandboxed`, a narrow write allowlist, a narrow network allowlist, read-only and CI commands pre-approved, and secrets denied. Per-user Claude files are git-ignored.

- Write paths (outside the repo and `$TMPDIR`, which are already writable): `~/.cargo/registry`, `~/.cargo/git` (crate cache and git dependencies), `~/.cargo/.package-cache` and `~/.cargo/.global-cache` (cargo lock and cache files), `~/.rustup` (toolchains and rustup metadata). The macOS `lipo`/xcrun cache lives under `$TMPDIR`, so nothing is added for it.
- Domains: `crates.io`, `index.crates.io`, `static.crates.io` (crate index and downloads), `static.rust-lang.org` (rustup toolchains), `github.com`, `codeload.github.com`, `objects.githubusercontent.com`, `raw.githubusercontent.com` (git dependencies and source archives), `api.github.com` (the `gh` PR workflow). `api.nuget.org` is only for `tools/dof_bridge` (the Windows .NET build); it is harmless on other machines.
- ssh remote check: `git remote -v` shows `git@github.com:donfuldauer/kilohertz-pinball.git`, an ssh remote. `~/.ssh/**` and `~/.config/gh/**` are denied to the Read tool through `permissions.deny`. Claude Code merges Read deny rules into the sandbox, so sandboxed commands cannot read `~/.ssh` or `~/.config/gh` either (verified: in a sandboxed shell `gh auth status` fails with "open ~/.config/gh/config.yml: operation not permitted" and `ssh-add -l` fails with "Error connecting to agent: Operation not permitted"). As a result, `gh` and git over ssh (fetch, pull, push) have to run outside the sandbox. An unsandboxed run prompts for approval unless it matches an allow rule: the read-only commands in `permissions.allow` (`gh pr view/checks/list`, `gh run list/view`, and the read-only `git` commands) run unsandboxed without a prompt, which is intended because they only read; the history- and remote-changing commands in `permissions.ask` always prompt. That is deliberate: credentials stay out of reach of sandboxed commands. `sandbox.filesystem.denyRead` keeps `~/.aws`, `~/.gnupg` and `~/.cargo/credentials.toml`; they are not removed because they are not exact duplicates of the `Read(...)` rules.
- Permissions: allow only read-only or CI commands from README "Building and testing" (`cargo fmt/clippy/test/build/check/metadata`, the packaging and gate scripts, read-only git, read-only `gh`). Verified against `scripts/`: `package.py`, `human_acceptance.py` and `gate_a2_verdict.py` are run with `python3`; `gate_a1.sh` and `gate_a2.sh` are executable bash scripts run as `./scripts/...`. No rule was dropped. `git commit`, `git push`, `gh pr merge` and the other history- or remote-changing git/gh actions are in `permissions.ask` because `autoAllowBashIfSandboxed` would otherwise approve them (omission from `allow` does not prevent auto-approval of sandboxed commands). `rm`, `curl` and `sudo` are not in `allow`.
- `allowUnsandboxedCommands`, `defaultMode`, `excludedCommands`, `network.allowLocalBinding`, `strictAllowlist`, `filesystem.disabled`, `allowAppleEvents` and `allowNestedUserNamespaces` are left unset.
- `.gitignore` gains `.claude/settings.local.json`, `.claude/worktrees/` and `.claude/.cc-writes/`.

**Rationale:**
- `~/.cargo` is not allowed wholesale because it holds `credentials.toml` (registry tokens) and `bin` (executables a sandboxed process could replace); the four subpaths are all cargo needs to build.
- `allowUnsandboxedCommands` is unset so the default applies; the owner may need an unsandboxed escape for installs into `/Applications`, and a shared file should not forbid or grant that for everyone.
- `defaultMode` is unset so each person keeps their own permission mode; the shared file carries only safe defaults.
- Personal overrides belong in `.claude/settings.local.json`, which is git-ignored.
- `allowWrite` and `allowedDomains` merge across settings sources (user, project, local), so a personal file adds to these lists rather than replacing them.
- `permissions.ask` lists `git commit/push/reset/rebase/merge/tag`, `git branch -D`, `git clean`, `gh pr merge/close/create`, `gh release` and `gh api`: explicit ask rules still prompt when a command is auto-allowed because it is sandboxed, so these Orchestrator-only actions are never run silently.
- Project settings cannot weaken the sandbox anyway, and the brief's list of unset keys keeps it that way.

**Alternatives:**
- Allow all of `~/.cargo`: rejected (credentials and `bin`).
- Set `excludedCommands` for cargo: rejected; it would run cargo outside the sandbox.
- Omit `~/.ssh` and `~/.config/gh` from the Read deny rules so sandboxed git and `gh` work: rejected; Read deny rules are merged into the sandbox, and keeping credentials unreadable to sandboxed commands is preferred over convenience.
- Allow `git commit`/`git push`/`gh pr merge`: rejected; they are in `ask` instead.
- deny instead of ask: rejected; the Orchestrator commits and merges in normal operation.
- Wildcard `*.githubusercontent.com` or `*.crates.io`: rejected in favour of the exact hosts observed.
