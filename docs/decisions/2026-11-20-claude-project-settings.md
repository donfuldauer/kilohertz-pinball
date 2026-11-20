## claude-project-settings — Claude project settings

**Decision.** Ship `.claude/settings.json` with the sandbox on and `autoAllowBashIfSandboxed`, a narrow write allowlist, a narrow network allowlist, read-only and CI commands pre-approved, and secrets denied. Per-user Claude files are git-ignored.

- Write paths (outside the repo and `$TMPDIR`, which are already writable): `~/.cargo/registry`, `~/.cargo/git` (crate cache and git dependencies), `~/.cargo/.package-cache` and `~/.cargo/.global-cache` (cargo lock and cache files), `~/.rustup` (toolchains and rustup metadata). The macOS `lipo`/xcrun cache lives under `$TMPDIR`, so nothing is added for it.
- Domains: `crates.io`, `index.crates.io`, `static.crates.io` (crate index and downloads), `static.rust-lang.org` (rustup toolchains), `github.com`, `codeload.github.com`, `objects.githubusercontent.com`, `raw.githubusercontent.com` (git dependencies and source archives), `api.github.com` (the `gh` PR workflow). `api.nuget.org` is only for `tools/dof_bridge` (the Windows .NET build); it is harmless on other machines.
- ssh remote check: `git remote -v` shows `git@github.com:donfuldauer/kilohertz-pinball.git`, an ssh remote. Therefore `~/.ssh` and `~/.config/gh` are omitted from the sandbox `denyRead` (git over ssh needs the keys, `gh` needs its token). The same paths are still denied to the Read tool through `permissions.deny`, so the agent cannot read them directly. `~/.aws`, `~/.gnupg` and `~/.cargo/credentials.toml` are in both lists.
- Permissions: allow only read-only or CI commands from README "Building and testing" (`cargo fmt/clippy/test/build/check/metadata`, the packaging and gate scripts, read-only git, read-only `gh`). Verified against `scripts/`: `package.py`, `human_acceptance.py` and `gate_a2_verdict.py` are run with `python3`; `gate_a1.sh` and `gate_a2.sh` are executable bash scripts run as `./scripts/...`. No rule was dropped. `git push`, `git commit`, `gh pr merge`, `rm`, `curl` and `sudo` are not allowed (the Orchestrator does git).
- `allowUnsandboxedCommands`, `defaultMode`, `excludedCommands`, `network.allowLocalBinding`, `strictAllowlist`, `filesystem.disabled`, `allowAppleEvents` and `allowNestedUserNamespaces` are left unset.
- `.gitignore` gains `.claude/settings.local.json`, `.claude/worktrees/` and `.claude/.cc-writes/`.

**Rationale.**
- `~/.cargo` is not allowed wholesale because it holds `credentials.toml` (registry tokens) and `bin` (executables a sandboxed process could replace); the four subpaths are all cargo needs to build.
- `allowUnsandboxedCommands` is unset so the default applies; the owner may need an unsandboxed escape for installs into `/Applications`, and a shared file should not forbid or grant that for everyone.
- `defaultMode` is unset so each person keeps their own permission mode; the shared file carries only safe defaults.
- Personal overrides belong in `.claude/settings.local.json`, which is git-ignored.
- `allowWrite` and `allowedDomains` merge across settings sources (user, project, local), so a personal file adds to these lists rather than replacing them.
- Project settings cannot weaken the sandbox anyway, and the brief's list of unset keys keeps it that way.

**Alternatives.**
- Allow all of `~/.cargo`: rejected (credentials and `bin`).
- Set `excludedCommands` for cargo: rejected; it would run cargo outside the sandbox.
- Deny `~/.ssh` and `~/.config/gh` in the sandbox: rejected for this ssh-remote setup; a user on an https remote may add them in `.claude/settings.local.json`.
- Allow `git commit`/`git push`/`gh pr merge`: rejected; the Orchestrator does git and merges.
- Wildcard `*.githubusercontent.com` or `*.crates.io`: rejected in favour of the exact hosts observed.
