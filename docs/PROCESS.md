# Kilohertz Pinball — Agent Execution Process

This file is the stable operating procedure for every agent that works on Kilohertz Pinball. It
does not change when the plan changes. The **plan** is the set of **GitHub issues** labelled
`work-unit`, grouped as sub-issues of the tracking issues #79 (M1 — Play experience) and #80
(M2 — Backlog): each issue is one unit of work, with its dependencies, requirements, owned paths
and acceptance criteria. The plan is never written into the repository. `docs/SPEC.md` says
*what* the engine is; `docs/DEVELOPMENT.md` holds the layout, commands, gates and contract owners;
this file says *how* an issue becomes a merged pull request.

Section numbers below (P1–P8) and rules (R1–R12) are what agents, briefs and replies cite.
D-numbers are the decision rows of `docs/SPEC.md` §0: D1–D33 and D43–D45 are architecture decisions; the
process rows D34–D42 point here, and their full text lives in this file. This file replaced
`docs/PLAN.md` §2.3 and §2.5–§2.7 on 2026-11-20 (DECISIONS.md "Repository — PLAN.md becomes
SPEC.md; process and layout move out"). The v5 build (subphases 0.1–7.2, Waves 0–9) predates the
issues; its record stays in `docs/RUN_LOG.md`. Where older text (SPEC §4, the run log,
`contracts/CHANGE_REQUESTS.md`) says *wave* or *subphase gate*, this process says *phase* and
*unit gate*.

---

## P1. Sources of truth

| Source | Role | Who writes it |
|---|---|---|
| `docs/SPEC.md` | The normative technical specification: architecture decisions D1–D33 and D43–D45 (§0), thread model (§2.2), determinism scope (§2.4), contracts (§3), component specifications with Gates A1 and A2 and the final human acceptance procedure (§4), library findings (§6), presentation and play-experience requirements with `REQ-<AREA>-<NNN>` IDs (§7), file schemas and reference tables (Appendices A–C, E, F). It was `docs/PLAN.md`; section, subphase and D-numbers are unchanged, so "PLAN §3.2" in code and history means SPEC §3.2. | A person, or a PR the person asked for (P8). Exception: the work unit that owns a new file schema or protocol documents it in SPEC Appendix A or B in its own PR (additive contract documentation, as CR-007 did with Appendix A.3a). |
| GitHub issues (`work-unit`) | **The plan.** One issue per reviewable PR. Title `KP-NNN: <title>`; first body line `> **Plan ID:** KP-NNN · **Phase:** … · **Milestone:** … (tracking issue #…) · **Expected tier:** …`; then Summary, **Depends on** (the only dependency record), Requirements (SPEC sections and REQ IDs), Deliverables, Acceptance criteria, Owned paths, Notes (`.github/ISSUE_TEMPLATE/work-unit.md`). Plan IDs are zero-padded and number the issues in topological plan order. Labels: status `ready` / `blocked` / `in-progress` / `status:blocked-failure`; `type:feature`, `type:fix`, `type:infra`, `type:docs`, `type:content`; `area:*`; `phase:NN-name`; `tier:T1` / `tier:T2` (expected tier; T3 is never planned); `review:gate` on an issue the plan designates to carry a gate review (P6). The retired per-PR review labels (`review:skip` / `review:sanity` / `review:full`) and any review-mode notes in older issue bodies are ignored. | `plan-from-spec`; new issues for discovered work by any Orchestrator, with dependencies filled in. |
| Tracking issues (`tracking`) | The milestones: #79 M1 — Play experience (must be complete before the owner repeats final acceptance) and #80 M2 — Backlog. Their sub-issues are the work units. A tracking issue is not a work unit (no Plan ID line) and closes when its last sub-issue closes. "Milestone" in this file and in the skills means a tracking issue. | `plan-from-spec`. |
| `docs/DEVELOPMENT.md` | Toolchain and pins, agent environment, workspace and crate map, exact commands, CI workflows, coverage floors, gate scripts, owned paths and shared files, contracts and their owners, cache and fixture paths. | Any unit whose change makes it stale updates it in its own PR. |
| `docs/PROCESS.md` | This file. | A person, or a PR the person asked for (P8). |
| `docs/BRIEF_TEMPLATE.md` | The delegation brief every agent receives. | Changed together with this file or the agent definitions. |
| `docs/DECISIONS.md` | Every ambiguity or judgement call an agent resolved, with rationale; the recommendation recorded there is the decision (R8). Also the decision history of the v1–v5 build. | Any agent (R8). Append only. |
| `docs/HUMAN_QUEUE.md` | Only the P8 items: things a person must do or decide, one line each, `- [P8 <kind>] [<id>] <text>`. Never judgement calls (those are decisions, R8) and never a request to close a review thread. Older lines stay as struck-through history with their disposition. | Any agent. Append only; resolved lines are struck through with the resolution and a link. |
| `docs/RUN_LOG.md` | One row per unit-of-work attempt, plus phase gates, merges, Fable asks and `[Fable wanted]` notes. | The Orchestrator. Append only. |
| `contracts/CHANGE_REQUESTS.md` | Requests to change a contract (P3) after its owning unit merged (R2). | Any agent. Append only. |
| `docs/briefs/` | Delegation briefs worth keeping (a phase-wide `COMMON.md`, long briefs). The Wave 5 and 6 briefs stay as history. | Work-classifier output, saved by the Orchestrator. |
| `docs/HUMAN_ACCEPTANCE.md` | The final human acceptance checklist (SPEC §4 Phase 8). | Generated by `scripts/human_acceptance.py`; never edited by hand. |

When SPEC and an issue disagree, SPEC wins, the work follows SPEC, and the disagreement is a
`docs/DECISIONS.md` entry. Because only a person edits SPEC (P8), a needed SPEC edit is one
`[P8 spec]` line in `docs/HUMAN_QUEUE.md`, except the additive schema documentation above.

**D41 is superseded.** D41 split new decisions and queue lines into one file per unit
(`docs/decisions/`, `docs/queue/`). There is again one decision log (`docs/DECISIONS.md`), the
human queue holds only P8 items, and the run log stays one file (DECISIONS.md "Process — One
decision log and a P8-only queue (D41 superseded)"). The shared files take appended entries only;
a merge conflict in one of them is resolved by keeping both sides, in order.

### Working with GitHub

Claude Code sessions (and Gemini / Antigravity sessions with a GitHub MCP server) use the GitHub
MCP tools (`mcp__github__*`) for issues, comments, labels and pull requests, and use the `gh` CLI
for searches that filter by label:
`gh api -X GET search/issues -f q='repo:donfuldauer/kilohertz-pinball is:issue label:<label> …' -F per_page=100 -F page=<N>`,
paged explicitly until a page returns fewer than 100 items (never `gh api --paginate`: a session
proxy can block the numeric-ID links it follows). Gemini / Antigravity sessions without GitHub MCP
tools use the official `gh` CLI (`gh issue`, `gh pr`, `gh run`, `gh api`) for all GitHub
operations, keeping the same explicit paging for `search/issues` when `--paginate` is blocked. The
same repository conventions apply in both harnesses (DECISIONS.md "Process — GitHub tooling: parent
tracking issues and body dependency lists"):

- **Grouping.** A milestone is a parent tracking issue (label `tracking`). A new work unit is
  attached to its tracking issue as a sub-issue (`sub_issue_write`, method `add`, or
  `gh api -X POST repos/donfuldauer/kilohertz-pinball/issues/<tracking>/sub_issues -F sub_issue_id=<id>`,
  with the new issue's numeric `id`, not its number).
- **Dependencies.** The issue body's **Depends on** list is the only dependency record. A
  dependency is satisfied only under the merged-PR rule (P6 routine step 3).
- **Claims.** A session claims an issue with a comment naming its branch and session ID. A claim
  is released by editing that comment (`update_issue_comment` or
  `gh api -X PATCH repos/donfuldauer/kilohertz-pinball/issues/comments/<comment_id> -f body='Released: <reason>'`)
  to `Released: <reason>`; a released claim does not count. Claim ordering is in the `work-issue`
  skill §1 step 4.
- **Labels.** Applied with `issue_write` (MCP `labels` sets the whole list, so read the current list
  first) or `gh issue edit --add-label ... --remove-label ...` / `gh api -X PUT .../labels`. Every
  label exists before the first issue that uses it is created.

## P2. Execution rules (R1–R12)

These apply to every change, by every agent. Briefs inline them in full.

### R1. Module boundaries (D3)
An agent edits only the paths its brief lists as OWNED PATHS (from the issue). Inside a shared
crate each unit owns a module directory; where two units that can run at the same time share a
crate, the earlier one pre-stubs the `mod` declarations. Anything outside the owned paths that
must change is reported as a request, never edited. The shared append-only files of P5 (unit gate
item 8) are allowed for every issue.

### R2. Contract-change protocol (D4)
All cross-crate types live in `pin_core`. A contract (P3) is written freely only by the unit that
owns it, until that unit merges (Subphase 1.1 wrote `pin_core`). Any later change is appended to
`contracts/CHANGE_REQUESTS.md` (next free `CR-NNN`, additive or breaking as defined there) and
lands as a separate contract-only PR (a `Contract: …` issue is such a PR). Additive changes merge
immediately; breaking changes merge only between phases, after the current phase gate and before
the next phase's issues start, with every crate that uses the changed item fixed in the same PR.
While a request is pending, the requesting agent continues with a local workaround that does not
edit the contract. Units are SI inside every contract (D17, SPEC §3.1); files use mm, g and
degrees and are converted at load.

### R3. Determinism (D9, SPEC §2.4)
The gate is same-build determinism: the same binary on the same machine replays any recording to
identical ball state, switch-hit counts and scores, plus the reference-set equality below. To stay
equal across platforms: no fast-math flags, no FMA contraction, no explicit fused or
SIMD-width-dependent float ops in the sim crates (quaternions through `pin_physics::quat_ops`,
`min`/`max` through `pin_core::fmath`), NaNs hashed as one canonical pattern, `libm` for transcendental functions (std transcendentals are a clippy
error in `pin_core`), fixed iteration order everywhere (no iteration over `HashMap` or `HashSet`
where order can affect a result), and a sim-owned seeded PCG for all randomness, recorded in
replays. Table rules logic is integer-only (SPEC §3.4 parity rule), so Lua and Wasm command
streams compare bit for bit. The `replay-hash` fixture `state_hash` and the seam digests must equal the committed x86_64-linux goldens on aarch64, Windows x86_64 and wasm32 (V8, JavaScriptCore, SpiderMonkey); other cross-OS hashes are reported. A change to
simulation output for every table (P3) re-baselines the replay digests once, in its own PR, and
re-runs Gates A1 and A2.

### R4. No blocking on the sim thread (D5)
No heap allocation, mutex or file I/O in `step_1ms()` in Rust engine code. Tests wrap the tick in
a panicking-allocator guard. Two explicit exemptions: the Lua VM allocates through mlua's capped
counting allocator with logical (LP64) accounting (KHZ 33); and a hot-reload swap tick may allocate
and overrun (developer-only event). In the web build the JS glue is preallocated, and the
allocation guard runs in the Node build of the web module.
The I/O worker is the only thread that touches the filesystem at runtime, and no `String` crosses
into the sim thread (interned `u16` IDs, SPEC §2.2, §3.1).

### R5. GPU-free core tests (D20)
`pin_core`, `pin_physics`, `pin_input`, `pin_script`, `pin_assets`, `pin_tablegen` and `pin_sim`
pass `cargo test` with no display server and no GPU. Render tests live behind the `gpu-tests`
feature and run in their own CI job (lavapipe on pull requests; lavapipe, WARP and Metal on
`main`). `pin_web`'s host-testable modules are GPU-free too; its wasm tests run in Node.

### R6. Simulated time in tests (D8)
Headless tests never sleep and never assert on wall-clock time. Wall-clock accuracy is a
non-gating benchmark, except where SPEC states a measurement in the windowed app (SPEC §7.5),
which is made in the windowed harness in the units SPEC gives (presented frames, dropped ticks).

### R7. No test-shaped special cases (D16)
Solver code must not branch on named behaviours ("live catch", "powerball"). Tests assert
emergent results across parameter sweeps.

### R8. Never wait on a person (D28)
When a brief is ambiguous, or a judgement call is needed, choose the option most consistent with
SPEC, the issue and `docs/DECISIONS.md`; record it in `docs/DECISIONS.md` as
`## <work item> — <title>` with **Decision**, **Rationale**, **Alternatives**; continue. An
agent's recommendation on a judgement call is the decision: it is recorded once, is final, needs
no confirmation, and is not added to `docs/HUMAN_QUEUE.md`. Only a later DECISIONS.md entry (or
the repository owner) changes it. `docs/HUMAN_QUEUE.md` is kept only for the P8 items an agent
cannot do or decide itself. Consequently: a `[Fable wanted]` note goes in the unit's
`docs/RUN_LOG.md` row; a CodeRabbit that never reviews a gate is a RUN_LOG row; a unit that fails P7 is
marked `blocked` in RUN_LOG with the `status:blocked-failure` label and an issue comment with the
failure history; a deferred review finding becomes a new `work-unit` issue or a DECISIONS.md
entry; a SPEC erratum is recorded in DECISIONS.md with one `[P8 spec]` queue line proposing the
SPEC edit (DECISIONS.md "Process — Judgement calls are decisions, not queue items").

### R9. No self-review (D36, D37)
No agent reviews code for style or design, its own or another agent's. The work-verifier checks
objective facts only. Code review is done by CodeRabbit, and only at the completion of a phase or
milestone, on that gate's cumulative review PR (P6 "Gate reviews"); routine unit-of-work PRs are
not code-reviewed. On a gate review the Orchestrator resolves every review thread itself when
justified and never leaves one for a person.

### R10. Tests are evidence, not obstacles
Never delete, skip, `#[ignore]` or weaken a test to get green; fix the code, not the test. A test
that contradicts the brief is corrected to assert exactly what the brief requires (never less),
and the report names the brief clause and the evidence (command and output) that the old
assertion was wrong.

### R11. Untrusted tables stay untrusted (D25)
Table bundles are untrusted downloads. Lua loads only `base` (without `dofile`, `loadfile`,
`load`), `string`, `table`, `math` (with `math.random` routed to the host PCG) and `utf8`; no `io`,
`os`, `debug` or `package`. A memory cap (counted in logical LP64 sizes, with a
real-byte backstop at 2x the cap), a per-callback instruction budget and the per-tick budget
(D45: 200 000 units shared by a tick's callbacks) abort runaway scripts, and an aborted script
drops into a safe tilt state without stalling the tick. Wasm rules run from the load-time
instrumented module (fuel and weighted call-depth counter, SPEC §3.4) on every platform: wasmtime
natively, wasmi on the web. Wasm runs without WASI, with a memory cap, fuel metering as the
per-callback budget (within the per-tick budget) and NaN
canonicalisation; fuel exhaustion drops the script into the same safe state. Zip extraction
rejects absolute paths, `..` components and symlinks, and caps total uncompressed size and
compression ratio (SPEC §3.5); the web adds a 32 MiB `File.size` check before any copy, web `ZipLimits` (64 MiB total, 32 MiB per file, ratio 100, 10 000 entries), no folder bundles and no stored bundles. The vendored Lua patches (`third_party/lua-src/KHZ_PATCHES.md`)
are part of this boundary. Every parser (TOML manifests, zip, glTF extras, UDP packets, replay
files, input profiles, and the audio decoders, including the vendored Ogg Vorbis decoder in
`third_party/lewton`) survives a bounded random-bytes run without panicking. Accepted risks are
recorded in `docs/THREAT_MODEL.md`.

### R12. Evidence for what a player sees and touches (D30, D31)
The v5 gates all ran headless and missed a one-second flipper delay and a view that did not look
like a machine (SPEC §7). So:
- **Image tests (D30)**, in this order: (a) analytic assertions on pixels, such as region mean
  colour, luminance ratios, symmetry and energy conservation; (b) a regression snapshot, stored
  only after (a) passes, with per-backend tolerance; (c) the work-verifier's visual checklist,
  walked on rendered images it actually opens, and logged. A snapshot is never the sole evidence.
- **Hardware simulation (D31).** Gamepad-mode devices are SDL virtual joysticks; keyboard-encoder
  devices are synthetic key events; plunger and accelerometer inputs are recorded traces stored as
  fixtures; displays are mocked; audio renders offline to buffers; telemetry uses loopback UDP.
  Anything that needs a physical device is a final-acceptance item (`docs/HUMAN_ACCEPTANCE.md`)
  and, when new, a `[P8 hardware]` line.
- **Windowed app.** Any change to input, timing or presentation is also verified in the real
  windowed app (Xvfb and lavapipe in CI and in agent sessions), never only headless. For web units,
  a real browser page with a presented canvas is the windowed evidence (the presented canvas
  equals the readback).

## P3. Contracts and blast radius

### Contracts
A *contract* is anything more than one crate, tool or file format depends on.
`docs/DEVELOPMENT.md` names the issue or subphase that owns each. At minimum:

- the `pin_core` types and the SPEC §3 contracts: IDs and units (§3.1), ball profiles (§3.2), the
  flipper contract (§3.3), the scripting contract (§3.4: `HostView`, `HostCommandQueue`,
  `HostCtx`, `TableScriptBackend`), the bundle format (§3.5), and `InputEvent`, `CabinetInput`,
  `CabinetAxis`, `HapticsTelemetryEvent`, `MaterialPair`, `SimStep`, the SPSC ring and the triple
  buffer;
- the file schemas of SPEC Appendix A (`table.toml`, `backglass.toml`, `layout.toml`,
  `glass.toml`, and every schema a later unit adds there);
- the UDP telemetry protocol (SPEC Appendix B, `docs/TELEMETRY_PROTOCOL.md`);
- `physics_defaults.toml` and its bounds (SPEC Appendix C);
- the replay file format;
- the `.pinbundle` layout;
- the asset manifest IDs of the placeholder kit (D32);
- the `FrameStateSnapshot` layout;
- the Lua `pinball` module and the Wasm `pinball` imports and guest exports;
- the guest SDK (`pin_guest_sdk`).

### Blast radius
- `local`: one module, covered by its own tests.
- `module`: several modules in one crate, or a public API used elsewhere.
- `very-high`: at least one of
  - a breaking change to a `pin_core` contract that three or more crates depend on;
  - a change to simulation output for every table: solver numerics in `pin_physics` world, ball
    or flipper, the sim tick, the triple buffer, the physics defaults, or the gravity direction
    (replay digests change);
  - a change at the untrusted-table security boundary (R11): the Lua or Wasm sandbox, budgets,
    memory caps, zip extraction, the vendored Lua patches;
  - a change one revert cannot undo: data or format migrations, release signing and packaging.

## P4. Model routing (D34, D38, D42)

Every unit of work (an issue's Implementer or Independent Tester task, a fix task, a CodeRabbit
finding, a CI or gate failure) goes through the **work-classifier** before anyone works on it,
except items that take the fast path below. The classifier rates blast radius and difficulty and
picks the lowest tier that can do the job.

### Tiers

| Tier | Claude Code (`model` override) | Gemini / Antigravity (`Model` override) | Typical work |
|---|---|---|---|
| T0 | Haiku 5.5 (`haiku`) | Flash-Lite (`flash_lite`) | Docs-only edits, run-log lines, formatting, renames, a review nit whose fix the comment spells out exactly |
| T1 | Sonnet 5.5 (`sonnet`) | Flash (`flash`) | Well-specified changes in one module, adding tests, CI and workflow tweaks, most review findings, verification runs |
| T2 | Opus 5.5 (`opus`) | Pro (`pro`, or `inherit` on a Pro/reasoning session) | New feature implementations, numerics, solvers, render pipelines, concurrency, cross-module changes, ambiguous work, every investigation |
| T3 | Fable 5.2 (`fable`) | High-tier escalation (`pro` at maximum reasoning) | Only under the T3 / Fable rule below |

An issue's `tier:` label is the expected tier for its Implementer; the classifier may route lower,
and each fix or finding is classified on its own. Verification runs on T1 (`Sonnet 5.5` / `Flash`)
and investigation runs on T2 (`Opus 5.5` / `Pro`) by default; neither needs to differ from the
implementing model. In Antigravity sessions where the user has not requested switching model tiers,
`invoke_subagent` may keep `inherit` when the harness policy requires it.

### Fable / T3 escalation rule (D34)
T3 (Fable 5.2 on Claude Code; maximum-reasoning `pro` on Gemini / Antigravity) is an exception. It
is used only when all three hold:
1. **Need.** The work has a *very-high blast radius* (P3), **or** it is *extremely hard to
   troubleshoot*: two full issue-investigator passes at T2 (`Opus 5.5` / `Pro`) found no
   reproducible root cause, or T2 used up the P7 fix attempts, or the failure is non-deterministic
   below the code's control (driver, compiler, platform) and two T2 passes did not isolate it.
2. **T2 is not enough.** The classifier names the T2 attempt that failed, or says exactly why one
   would.
3. **Allowance / quota.**
   - **Claude Code:** Just before routing, the Orchestrator sets `FABLE_ALLOWANCE`. It is
     `insufficient` if the session reports a current rate limit, throttling or fallback on Fable;
     otherwise the Orchestrator cannot read it (the agent has no access to the `/usage` readout),
     so it runs the Fable ask below (D42). Only an owner `yes` within the ask's 30-minute window
     makes it `sufficient`. Without that, it is `unknown`, which counts as not enough.
   - **Gemini / Antigravity:** T3 escalation requires the active plan and quota to support it (no
     rate limit, throttling or fallback). If unavailable or unconfirmed, run at T2.

If 1 and 2 hold but 3 does not, the work runs at T2 (`Opus 5.5` / `Pro`) and a `[Fable wanted]`
(Claude Code) or `[T3 wanted]` (Gemini / Antigravity) note saying what T3 would have been used for
goes in that unit's `docs/RUN_LOG.md` row; no queue line is written. Apart from the Fable ask's
bounded wait, the run never waits for allowance to return. If a T3 session is throttled or falls
back mid-task, the work continues at T2. Every T3 use is logged in `docs/RUN_LOG.md` with the
classifier's evidence for 1–3. When torn between two tiers, take the higher one, but never move up
into T3 that way.

### Fable ask (D42, Claude Code)
In Claude Code, the agent cannot read the account's usage, so when the classifier returns
`fable check: wanted, allowance unknown, owner ask` for one or more items (DECISIONS.md
"Process — Fable ask kept"):
1. The Orchestrator first schedules a reminder into its own session 30 minutes later
   (`send_later` with `delay_minutes: 30`, or the session's equivalent) saying the ask for those
   items timed out. If the session cannot schedule one, it skips the ask and treats the allowance
   as `unknown`.
2. It then asks once per batch, as a chat message to the owner (plus a push notification when the
   session has a notification tool), naming each item with its issue number and plan ID (or PR and
   thread for a finding), the classifier's evidence for 1 and 2, and a rough size (agent runs,
   hours), and asking the owner to check `/usage` and reply `fable yes` or `fable no`, for all
   items or per item. It does not use a blocking question dialog, because that has no timeout. If
   the ask cannot be sent, it cancels the reminder and treats the allowance as `unknown`.
3. While waiting it continues any work that does not depend on those items; if there is none, it
   ends its turn.
4. Answers are per item. On `fable yes`, the Orchestrator re-checks the session first: if a rate
   limit, throttling or Fable fallback has been reported since the ask, it passes
   `FABLE_ALLOWANCE: insufficient`; otherwise it re-runs the work-classifier for the items that
   answer approves (all items if the owner answered for all, otherwise only those named as yes)
   with `FABLE_ALLOWANCE: sufficient (owner, <UTC time>)`, which may route them to T3. On
   `fable no` an item is `insufficient` (even if another item in the same ask got `yes`): the T2
   brief already returned is used and the `[Fable wanted]` note goes in the unit's RUN_LOG row. A
   partial answer does not drop the timeout: the reminder stays active until every item in the ask
   has an outcome, and is cancelled only then. When it fires, every item still without an answer
   is `unknown` (T2 brief, `[Fable wanted]` note).
5. A `yes` covers only the items it approves in that ask and does not carry over to later issues;
   an answer after the timeout does not re-route work already routed. The ask, the answer or the
   timeout, and the outcome go in `docs/RUN_LOG.md`.

### Fast path (D38)
The Orchestrator classifies an obviously small item inline with the `triage` skill
(`.claude/skills/triage/SKILL.md`, forwarded by `.agents/skills/triage/SKILL.md`) instead of
spawning a work-classifier. The skill applies only when all of these hold: the item is a fix, a
CodeRabbit finding, a docs or run-log edit or a CI tweak; its tier is clearly T0 or T1; it touches
one module or docs only; it needs no `pin_core` contract change, no change to simulation output and
no change at a security boundary; its validity and cause are clear; and the fix is spelled out or
obvious. If its validity or cause is unclear, the item goes to the issue-investigator first; by its
verdict, `valid-defect` is re-triaged from its report, `valid-but-deferred` or `pre-existing` takes
the "Deferred" disposition (unless it is an unfixed security issue or a verified unfixed correctness
defect: then it goes to the classifier and the thread stays unresolved, P6 "Gate reviews"
step 4) and `not-a-defect` takes "Not a defect" (P6 "Gate reviews" step 4), with no Fixer for
either; if any other check fails, or the Orchestrator is unsure, it goes to the work-classifier. The fast path never routes T2 or T3,
and the classifier still handles every issue implementation, every Independent Tester task, every
investigation, every gate failure and anything else outside the fast path. Fast-path rows in
`docs/RUN_LOG.md` start the Notes column with `triage`, followed by the tier and a one-line reason.

## P5. Agents and the flow of one unit of work (D35, D38, D39)

### Agents (D35)
Canonical agent prompts live in `.claude/agents/` (loaded directly by Claude Code); Antigravity
custom subagents live in `.agents/agents/` and forward to them so role instructions stay in one
place. In Antigravity environments that do not auto-load `.agents/agents/`, the Orchestrator
registers each role with `define_subagent` (using the prompt from `.claude/agents/<name>.md`, with
write tools enabled only for `task-executor`) and launches it with `invoke_subagent`. (Standalone
Gemini CLI does not discover `.agents/agents/`; see `GEMINI.md`.) Subagents never spawn subagents
(enforced by Claude Code and by policy in Gemini / Antigravity), so the Orchestrator (the main
session) does every spawn, passing the classifier's brief **verbatim** and its model tier override.
The Orchestrator writes no product code and reviews no code.

| Agent | Default tier (model) | Does | Never does |
|---|---|---|---|
| `work-classifier` | T1 (`Sonnet 5.5` / `Flash`) | Rates blast radius and difficulty, picks the tier (P4), names the agent, writes the self-contained brief (`docs/BRIEF_TEMPLATE.md`); may take a batch and return one routing block per item and one brief per executor group | Edit files, implement, review |
| `task-executor` | from the classifier | *Implementer* (deliverables and unit tests), *Independent Tester* (black-box and property tests from the brief alone, in parallel with the Implementer, never seeing the implementation) or *Fixer* (one CodeRabbit finding, verifier failure or investigator plan), strictly from the brief | Leave its owned paths, review code |
| `work-verifier` | T1 (`Sonnet 5.5` / `Flash`) | Re-runs every gate step; traces each requirement and REQ ID to a passing test; checks owned paths, the sim-thread rules and the R12 evidence, opening the rendered images itself | Give style or design opinions, edit code |
| `issue-investigator` | T2 (`Opus 5.5` / `Pro`) | Reproduces and root-causes any failure whose cause is not obvious; judges CodeRabbit findings of unclear validity; returns a fix plan and a thread reply | Change repository files, fix |

### Flow
classify (inline via `triage` for eligible small items) → execute (Implementer and Independent
Tester in parallel for an issue with code) → verify → on `fail`, investigate if the cause is
unclear, then classify and execute the fix → verify again → open the PR (P6) → merge on green CI
and a passing verifier. When the issue completes a phase or milestone, the gate review (P6) runs
first and its CodeRabbit findings go through the same loop until the gate merge conditions hold.

### Batching (D38)
One gate-review round or one push is one batch: the Orchestrator lists every new finding;
sends findings of unclear validity or cause to the issue-investigator first (then re-triages
`valid-defect` ones; `valid-but-deferred` and `pre-existing` ones are deferred, except unfixed
security or verified correctness defects, which go to the classifier; `not-a-defect` ones are
answered as such, with no Fixer); takes the obviously small ones through `triage`; moves any
fast-path item that shares a file with a classifier-bound item into the classifier batch; sends
all classifier-bound items to one work-classifier call; runs one Fixer brief per (tier, module
directory) group, with docs-only items as one docs group (items of different tiers on the same
file go in one brief at the higher tier); and runs one work-verifier on the resulting head commit
covering every item whenever the round changed the head (a log-only round uses the verifier's
log-only case). It skips the verifier only when the head is unchanged; the previous pass on that
head stands.

### Briefs
The work-classifier writes each brief from `docs/BRIEF_TEMPLATE.md` so it is self-contained: the
issue's requirements and acceptance criteria, the rules R1–R12, the relevant SPEC sections,
contracts and appendices, the commands and environment from `docs/DEVELOPMENT.md`, and the owned
paths. Role agents never need SPEC as a whole, the issue tracker or this file.

### Branches and claims (D39)
`main` is protected. Each unit of work is one GitHub issue and runs on its own branch from the
latest `main` (`issue/<N>-<slug>` in its own worktree `../kilohertz-issue-<N>`, or directly in the
checkout when a sandbox restricts sibling directories and only one issue runs in the VM) and
reaches `main` only through its own PR, which closes the issue (`Closes #<N>`). One issue, one
branch, one PR. Work found along the way that the issue cannot absorb becomes a new `work-unit`
issue with its dependencies filled in, attached to its tracking issue. The `issue/` prefix is
required when GitHub-triggered session chaining is enabled: merging an `issue/` branch starts the
next session (`work-issue` §8). If the session can push only to one assigned branch, the PR body
says so and that the chain will not continue from this merge, and commit subjects carry the work
item (`[#<N>] …`). A session claims an issue before working it and releases the claim by editing it
(P1 "Working with GitHub"). Git tags are not used (D39): a session git proxy may refuse tag pushes,
and the phase-gate row in `docs/RUN_LOG.md` (commit SHA and CI run) is the record. Release tags
`v*` are pushed by the owner only (P8).

### Unit gate (definition of done)
All must hold:
1. `cargo fmt --all --check` and `cargo clippy --all-targets -- -D warnings` are clean on the
   touched crates (with `--features gpu-tests` for the render crates); commands in
   `docs/DEVELOPMENT.md`.
2. Implementer tests and every acceptance criterion in the issue pass; every SPEC section and REQ
   ID the issue lists is traced to a passing test or to its R12 evidence.
3. The Independent Tester's spec tests pass. A disagreement between spec tests and implementation
   is resolved against the brief, not by deleting the test (R10).
4. Line coverage (`cargo-llvm-cov`): at least 85 % for `pin_core`, `pin_physics`, `pin_input`,
   `pin_script`, `pin_assets`, `pin_tablegen`, `pin_sim`; at least 60 % for `pin_render`,
   `pin_audio`, `pin_app`; measured on the touched crates and the owned modules. A new public
   function with no test fails even when the crate total passes.
5. Property tests (`proptest`) cover every solver invariant and numeric helper; every parser
   touched (TOML manifests, zip, glTF extras, UDP packets, replay files) survives a bounded
   random-bytes run without panicking.
6. For anything on the sim thread: the allocation guard passes and a replay hash is stable over 10
   runs.
7. For render work, the D30 order and the logged visual checklist; for input, timing or
   presentation work, the windowed-app evidence (R12).
8. An automated path check confirms the diff touches only owned paths plus the shared append-only
   files `docs/RUN_LOG.md`, `docs/DECISIONS.md`, `docs/HUMAN_QUEUE.md` and
   `contracts/CHANGE_REQUESTS.md`, and new exact pins in the root `Cargo.toml`
   `[workspace.dependencies]` under a comment naming the issue.
9. The work-verifier reports `pass`. The unit is done only when its PR merges under P6.

### Phase gate (D39, D40)
A phase is the set of issues with one `phase:NN-name` label. When every issue of the phase is
complete under the merged-PR rule (P6 routine step 3), or optional and blocked under P7, the
Orchestrator runs the phase gate on `main`: the full workspace tests on all three operating systems (the full
matrix runs on every push to `main`, D40, and the gate reads it there); replay hashes stable;
every earlier phase's tests still pass; the tick-time benchmark has not regressed by more than
10 % against the previous phase gate's commit (method in `docs/DEVELOPMENT.md`); every
automated gate the phase owns passes; and the phase's gate review (P6), run on the phase's last
issue before it merges, is approved by CodeRabbit with every thread dispositioned. A milestone
gate is the phase gate of the tracking issue's last open sub-issue plus the milestone's exit
criteria. A green phase is recorded by its phase-gate row in
`docs/RUN_LOG.md` (commit SHA and CI run), committed with the next PR; no tag (D39). A red phase
gate turns each failure into a new issue against the owning area (P7); it never pauses for a
person.

### Automated gates
Gate A1 (physics reference suite on the grey box) and Gate A2 (table soak) are normative in SPEC
§4 Phase 6; Gate A3 (play experience in the real windowed app, SPEC §7.7, REQ-GATE-001) is
delivered by KP-043 (#124) and runs on `main` after M1's table work. Commands are in
`docs/DEVELOPMENT.md`. A1 and A2 re-run whenever a change alters simulation output for every
table (P3), and A2 whenever table rules change. A failing gate opens fix issues against the owning
area under P7; it never pauses for a person.

## P6. Pull request, review and merge (D36, D37, D40)

Agents do not review their own code (R9). CodeRabbit is the only code reviewer, and it reviews
**only at the completion of a phase or milestone**, never on every unit of work. Routine PRs merge
on objective gates; a gate review covers, in one cumulative pass, everything merged since the
previous gate. The Orchestrator drives every PR to a merge without a person.
(`.coderabbit.yaml` keeps `auto_review.enabled: false`; every CodeRabbit pass is requested
explicitly on a gate review PR.)

### Routine PRs (every unit of work)

1. **Open.** The Orchestrator commits the unit's `docs/RUN_LOG.md` row(s) before the
   work-verifier's final run, so the verified commit is the PR head. When the verifier passes, it
   pushes the branch and opens a PR against `main` (`create_pull_request` or `gh pr create`). The
   body follows `.github/pull_request_template.md`, starts with `Closes #<issue>`, says under
   `## Work item` whether the issue is a gate issue (below), and fills every section (write "not
   applicable" with a reason rather than leaving one empty): requirements with the test that
   proves each one, gate results with numbers, visual and windowed evidence, decisions logged,
   contract-change requests, and every T3 / Fable use with its P4 evidence. Bot names in the body
   go in backticks so nobody is notified by accident. No `review:*` label is applied and no
   CodeRabbit review is requested.
2. **PR watch / auto-fix.** Right after opening:
   - **Claude Code:** subscribe to the PR's activity (`subscribe_pr_activity`: comments, reviews,
     CI). If the session cannot subscribe, schedule `send_later` check-ins about every 3–5 minutes
     while waiting on CI until the PR merges.
   - **Gemini / Antigravity:** run the `pr-watch` skill (`.agents/skills/pr-watch/SKILL.md`),
     which schedules non-blocking check-ins (`/schedule`, a single async background check-in, or a
     persistent `/automation` job across sessions), handles CI failures and repeats until the PR
     is ready to merge.

   Every CI failure is handled as it arrives or on check-in through the P5 loop; nobody
   sleep-polls in a foreground loop. If CodeRabbit posts on a routine PR anyway (a person asked
   it to), its findings are accounted for under "Findings" below before merging.
3. **Merge conditions.** The Orchestrator merges, without waiting for a person, when all of these
   hold on the current head commit:
   - every PR check is green (D40: the Linux set), i.e. the single required status check
     `pr-checks`, which aggregates the Linux set (`changes`, `fmt`, `clippy`, `core-tests`,
     `replay-hash`, `gpu-tests`, `dof-bridge`) and fails if any of them failed or was cancelled,
     or if `changes` did not succeed; a workflow outside `ci.yml` that ran on the PR (for example
     `rules-wasm`, `spikes`, `windowed`, or the `release` dry run) is a PR check too and must be
     green;
   - the branch is up to date with `main` whenever merging `main` in would touch a file the PR
     also changes or conflicts; a clean merge that touches none of the PR's files does not require
     a re-sync;
   - `main`'s last push CI is green; if it is red, only a PR that fixes or reverts the failing
     `main` run may merge (it must still meet every other condition here), and all other merges
     stay blocked until `main` is green;
   - the work-verifier's last result is `pass` for the head commit;
   - the issue is not a gate issue, or its gate review is approved (below).

   Then the Orchestrator merges with a merge commit (`merge_pull_request`, method `merge`, or
   `gh pr merge <P> --merge --match-head-commit <sha>`), checks the branch is deleted (deleting it
   if not), unsubscribes from the PR if subscribed, and **unblocks dependents**: every open
   `work-unit` issue whose **Depends on** entries are now all satisfied swaps its `blocked` label
   for `ready`, with a comment "Unblocked by #<N> (PR #<P>)". The **merged-PR rule:** a dependency
   is satisfied only if it is closed as completed by a merged PR (via MCP: `issue_read`
   `closed_by_pull_requests.references` + `pull_request_read` `merged` true; via `gh` CLI:
   `gh issue view <n> --json state,stateReason,closedByPullRequestsReferences` + `gh pr view <p> --json mergedAt`
   (`mergedAt` non-null); fallback for an unlinked PR body saying `Closes #<n>` or `Fixes #<n>`:
   run
   `gh api -X GET search/issues -f q='repo:donfuldauer/kilohertz-pinball is:pr is:merged base:main in:body "Closes #<n>"' -F per_page=100 -F page=<P>`,
   paged explicitly until a page returns fewer than 100 items, filter to PRs whose body contains a
   whole-number match for `Closes #<n>` (`#<n>` not followed by another digit, e.g. `#12` must not
   match `#123`), and only if no matching PR remains after paging and whole-number filtering run the
   same explicitly paged and whole-number-filtered query for `"Fixes #<n>"`), or a comment on it
   names the replacement
   issue(s) that delivered the work and those pass the same test. The run-log rows were committed
   in the PR itself; the merge commit is recorded in the RUN_LOG row of the next PR. Nothing is
   pushed to `main` directly.
4. **Never**, to get green: skip, ignore, delete or weaken a test; force-push or rewrite history on
   a branch the agent did not create; push an empty commit; close and reopen a PR to restart CI;
   merge with a failing required check.

### Gate reviews (completion of a phase or milestone)

**Gate issue.** An issue is a gate issue when, at the time its PR is ready to merge, it is the last
open `work-unit` issue carrying its `phase:NN-name` label (a **phase gate**) or the last open
sub-issue of its tracking issue (a **milestone gate**, for example the end of M1 `#79`; it is
usually both), or when the plan labels it `review:gate` (for example the M1 cohesive review
`#153` KP-064, which runs after Gate A3 `#124`). Check with
`gh api "repos/donfuldauer/kilohertz-pinball/issues?labels=work-unit,<phase-label>&state=open&per_page=100&page=N"`
(and the tracking issue's open sub-issues), paging explicitly (P1). The gate's name is the phase
label without its prefix (for example `12-rendering`) or the milestone (`M1`).

**Review PR.** A gate review is one review-only pull request whose diff is everything merged since
the previous gate review. Git tags are not used (D39), so the reviewed commit is recorded in
`docs/RUN_LOG.md`:

1. **Base.** The base commit is the `reviewed <sha>` recorded in the newest gate-review row of
   `docs/RUN_LOG.md` (if the latest gate's merge is not recorded yet, the merge commit of that
   gate issue's PR); if there is none, it is `4113e42` (the last commit of the former per-PR
   review rhythm). Push a branch `review-base/<gate>` at that commit.
2. **Open.** Once the gate issue's own PR passes its verifier, open a second PR with the gate
   issue's branch as head and `review-base/<gate>` as base, titled
   `Gate review: <phase or milestone> (do not merge)`, whose body links the gate issue's PR, lists
   the issues closed since the base, and states the phase or milestone exit criteria (SPEC
   sections, automated gates). Post `@coderabbitai full review` on it. Both PRs share one head
   branch, so every fix pushed for the review also lands in the gate issue's PR.
3. **Too large.** If CodeRabbit reports the diff exceeds what it reviews in one PR, split the
   review by area: for each group of top-level paths (for example `crates/pin_physics`,
   `crates/pin_render`, `tables/`), push `review/<gate>-<area>` built from the base plus
   `git checkout <gate head> -- <paths>`, open it against `review-base/<gate>` the same way, and
   refresh it after each fix push. Every area PR must be approved.
4. **Findings.** Each review round is one batch (P5 "Batching"), for every CodeRabbit comment
   (inline, nitpick, outside-diff or summary item). Each ends in exactly one disposition, recorded
   as a reply on its thread (`add_reply_to_pull_request_comment` or
   `gh api -X POST repos/{owner}/{repo}/pulls/{n}/comments/{comment_id}/replies -f body='<reply text>'`):
   - **Fixed:** an executor commit on the gate branch addresses it (gate-review fixes may touch
     paths outside the gate issue's owned paths; contracts still change only under R2), the
     verifier re-runs the affected checks, and the reply names the commit. The thread is then
     resolved.
   - **Not a defect:** the reply gives the evidence that the finding is not reproducible in the
     reported environment, already handled, out of scope, a documented accepted bound, or declined
     by repo convention (test output, commit SHA, the DECISIONS.md section, the PROCESS.md or SPEC
     rule, the `.coderabbit.yaml` setting, the doc line). A result from a different environment
     goes to the issue-investigator, and the thread stays unresolved until it returns a verdict.
     The thread is then resolved; if CodeRabbit argues back with new technical information, the
     finding goes round again.
   - **Deferred:** real but not needed to close the gate (non-blocking refactors, polish,
     maintainability). It becomes a new `work-unit` issue in a later phase or milestone (Depends on
     filled in, attached to its tracking issue) or a `docs/DECISIONS.md` entry that records why it
     is not done now; the reply links it, and the thread is resolved. Never a queue line, unless
     the deferral itself needs a P8 action (for example a SPEC edit). Security issues (R11),
     verified correctness defects, R3/R4 violations and contract mismatches are never deferred: a
     fix too large for the gate branch becomes a new issue the gate issue depends on, and the gate
     waits for it to merge, then merges `main` into the gate branch.

   Nitpicks are fixed when cheap (T0/T1), otherwise answered as above. No thread is resolved
   without a reply.

   **Resolving (D37).** The Orchestrator resolves threads itself, on its own judgment, with the
   thread resolve action (`resolve_review_thread`, GraphQL `resolveReviewThread` via
   `gh api graphql`, or the REST route
   `POST /repos/{owner}/{repo}/pulls/{n}/ccr/comments/{comment_id}/resolve`, threads listed with
   `GET …/pulls/{n}/ccr/review_threads`), when a fix landed and the verifier passed on it, when the
   reply gives evidence that the finding is not reproducible in the reported environment, already
   handled, out of scope or a documented accepted bound, when the finding is declined by repo
   convention (a DECISIONS.md entry, a PROCESS.md or SPEC rule, a `.coderabbit.yaml` setting), or
   when it was deferred and the reply links a new issue or a DECISIONS.md entry. Every reply states
   the reason and the evidence first (test output, commit SHA, decisions section, doc line), then
   anything else. It does not wait for CodeRabbit to accept, agree or mark the finding addressed. A
   person is never asked to resolve a thread. **Never resolved:** a thread that reports an unfixed
   security issue, or a verified correctness defect not yet fixed; it blocks the merge and goes
   through classify, fix, verify. A thread whose validity is unclear goes to the
   issue-investigator first; a failed reproduction in a different environment makes validity
   unclear. Where the thread type cannot be resolved (summary or outside-diff items), the reply
   alone is the disposition.
5. **Re-review.** Batch fix commits locally and push once per round, then post
   `@coderabbitai review` on the review PR. Merging `main` into the gate branch (to stay up to
   date) brings in whatever else merged meanwhile, which CodeRabbit then reviews too. If
   CodeRabbit has not reviewed the head commit (paused or rate-limited), ask once more, wait as
   long as it says, and ask at most twice more; if it still has not reviewed, both PRs stay open,
   a `docs/RUN_LOG.md` row records it, and the run continues with work that does not depend on
   them. When every thread is resolved or meets a resolve condition above, every other condition
   holds, and CodeRabbit's approval is what is needed to clear "Automatic approval withheld",
   post a plain comment `@coderabbitai approve` on the review PR.
6. **Gate merge conditions**, on the shared head commit: the routine merge conditions above;
   CodeRabbit has finished reviewing that head on every review PR of the gate, with an effective
   approval (no outstanding change request), no actionable comment unanswered, and every thread
   dispositioned and resolved (except types that cannot be resolved, where the reply is the
   disposition); and the P5 phase gate (and, for a milestone, its automated gates and exit
   criteria) holds.
7. **Close out.** Merge the gate issue's PR into `main`. Close every review PR **without
   merging**, with a comment linking the merge commit. Delete the `review-base/<gate>` and any
   `review/<gate>-*` branches (if the session proxy refuses branch deletion, leave them; they are
   inert). The gate's `docs/RUN_LOG.md` row (review PR links, rounds, deferred issues) is committed
   on the gate branch before the final verifier run; the next PR's RUN_LOG row records
   `reviewed <merge sha>` for the gate, which is the base of the next gate review. Nothing is
   pushed to `main` directly. Then unblock dependents.

Gate reviews never run in parallel: a second gate that becomes ready while one is open waits for
the first to close out, so its base is the first gate's reviewed commit.

## P7. Failure policy (D29)

1. The executor gets up to three fix attempts, each from a classifier brief built on the
   verifier's failures, the investigator's report or the CodeRabbit findings.
2. Then the issue-investigator makes a fresh pass, and the classifier routes one more attempt one
   tier up from a fresh brief plus the failure history. This can reach T3 (Fable 5.2 on Claude Code
   or high-tier escalation on Gemini / Antigravity) only if the P4 T3 / Fable rule holds, including
   allowance/quota; otherwise it stays at T2 (`Opus 5.5` / `Pro`).
3. Then the unit is marked `blocked` in `docs/RUN_LOG.md`; the issue gets the
   `status:blocked-failure` label (and loses `in-progress`) and a comment with the failure
   history; the session releases its claim. A person clears the label by replacing it with
   `ready`.
4. A blocked issue outside M1 (#79), or one that only adds an optional or restored feature (SPEC
   §0 "Restored features"), is skipped along with its dependents; the run continues.
5. A blocked *core* issue (an M1 issue on the way to Gate A3) halts only its dependents. All
   independent work continues, and the run ends with a partial result and a precise account of
   what is missing.

## P8. What does wait for a person

The run is unattended except for these. They are the only items ever added to
`docs/HUMAN_QUEUE.md`, one line each, `- [P8 <kind>] [<id>] <text>`, where `<id>` is the work item
that raised it (`KP-NNN`, `#<issue>` or `PR #<n>`). They never block independent work. Judgement
calls are decisions (R8).

- `spec`: edits to `docs/SPEC.md` (except the additive schema documentation of P1) or to this
  file;
- `hardware`: physical hardware checks (X-Arcade in each mode, analog plunger, accelerometer
  nudge, rumble, SSF channels, multi-monitor layout, DOF devices, a ray-tracing-capable GPU) (D31,
  D33);
- `assets`: replacing placeholder sounds, music, callouts and art with real assets by asset ID
  (D32);
- `release`: publishing a release (pushing a `v*` tag, reviewing and publishing the draft
  prerelease, signing and notarizing);
- `legal`: licensing and third-party assets;
- `setting`: repository settings an agent cannot change: branch protection and the required
  checks (`pr-checks`), the CodeRabbit app and its settings (its `path_filters` keep binary,
  generated, vendored and recorded-data files out of review), Actions runners (including an
  optional self-hosted ray-tracing runner, D33), secrets, and deleting branches the session's
  proxy refuses to delete.

Final human acceptance (SPEC §4 Phase 8, with REQ-GATE-002) is the person's review of the run as
a whole: feel, hardware, visuals on real displays, and real assets, worked through the generated
`docs/HUMAN_ACCEPTANCE.md` after M1 is complete. The owner reverses any decision they disagree
with as a follow-up issue.
