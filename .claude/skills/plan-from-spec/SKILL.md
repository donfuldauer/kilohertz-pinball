---
name: plan-from-spec
description: "Turns docs/SPEC.md requirements into Kilohertz Pinball's plan, which is a set of GitHub work-unit issues (one reviewable PR each) grouped under tracking issues, with 'Depends on' lists, labels and full SPEC section and REQ-ID traceability. Asks the owner only for P8 items the plan needs (SPEC edits, settings, hardware, assets, release, legal). Use when SPEC gains or changes requirements, when a tracking issue needs its work units, or when there are no ready work-unit issues left."
---

# plan-from-spec

The plan turns SPEC requirements into units of work the agents in `.claude/agents/` (forwarded by
`.agents/agents/` for Gemini / Antigravity) can execute under `docs/PROCESS.md`. **The plan is
GitHub issues, never a repository file**: each `work-unit` issue is one unit, sized as one
reviewable PR, and its body is the unit's spec. Every later brief is built from an issue, so issues
must be concrete: owned paths, deliverables, acceptance criteria.

Repository: `donfuldauer/kilohertz-pinball`. GitHub through MCP tools (with `gh api -X GET
search/issues` paged explicitly for label searches) or through the `gh` CLI when MCP tools are not
configured (PROCESS.md P1 "Working with GitHub").

## 1. P8 items for the owner (ask before planning, only when needed)

Kilohertz Pinball has no architecture questionnaire: its architecture is decided in SPEC §0
(D1–D33), and judgement calls while planning are decisions (R8), recorded in
`docs/DECISIONS.md`. Read `docs/SPEC.md`, `docs/DECISIONS.md` and `docs/HUMAN_QUEUE.md`. Ask the
owner only for the P8 items (PROCESS.md P8) the planned work cannot do without:

- `spec`: the work needs requirements SPEC does not state, or SPEC is wrong. New requirements get
  stable IDs `REQ-<AREA>-<NNN>` (testable SHALL-style statements in the section they belong to;
  existing sections keep being cited by section or subphase number). The SPEC change is a
  person's change: the owner approves the diff before it is committed. SPEC is not versioned
  inside the file (git is its history); a removed requirement keeps its ID as a one-line
  `[WITHDRAWN]` entry.
- `setting`, `hardware`, `assets`, `release`, `legal`: a repository setting (for example a new
  required check), a physical device, real assets, publishing, or licensing the plan depends on.

Ask with `AskUserQuestion` (Claude Code) or `ask_question` (Antigravity) / the harness's
interactive question tool (at most four questions per call; the recommended option first, marked
Recommended, with a one-line consequence per option). Record each answer's rationale in
`docs/DECISIONS.md` (`## Planning — <topic>`). If the owner is unreachable, append each open
question as a `- [P8 <kind>] [<id>] <text>` line in `docs/HUMAN_QUEUE.md`, plan everything that
does not depend on it, and leave the rest out until it is answered: P8 items never block
independent work.

## 2. Design the work units

Before creating anything, write the breakdown out (in the session's scratch space, not the
repository) and check it.

**Sizing.** One unit is one PR a person would review in one sitting: one coherent change in one
area, usually under about 800 changed lines excluding generated files and fixtures. Split
anything larger.

**Dependencies.** Only real prerequisites (code the unit calls, a contract it implements against,
a tool it runs), so independent units can run in parallel. The graph must be acyclic, and units
are numbered `KP-NNN` (zero-padded, next free number) in a topological order (that is the plan
order).

**Contracts first (R2).** A change to `pin_core` or another contract (PROCESS.md P3) is its own
`Contract: …` unit that appends its `CR-NNN` to `contracts/CHANGE_REQUESTS.md` and comes before
its first consumer; a new file schema's owning unit documents it in SPEC Appendix A in its own
PR. A unit that changes simulation output for every table (very-high blast radius) says so, and
its acceptance criteria re-baseline the replay digests and re-run Gates A1 and A2. Units that
change input, timing or presentation carry windowed-app acceptance criteria; render units carry
analytic pixel assertions and a visual checklist (R12).

**Issue body** (`.github/ISSUE_TEMPLATE/work-unit.md`): the Plan ID line (`KP-NNN`, phase,
milestone with its tracking issue, expected tier); Summary; **Depends on** (issue numbers with
plan ID and title, or "None: can start immediately"); **Requirements** (each REQ ID with the
SPEC section, or the SPEC section or subphase for older parts); **Deliverables** (checkboxes);
**Acceptance criteria** (testable, checkboxes, ending with "The unit gate in `docs/PROCESS.md` P5
passes."); **Owned paths**; **Notes** (out of scope, decisions to record); the closing line
"Start work on issue #<n>". A unit that changes the layout, commands or contracts lists
`docs/DEVELOPMENT.md` among its owned paths. No third-party pinball names (manufacturers,
machines, games, products, websites) anywhere in an issue: state values and generic names, never
the source (DECISIONS.md "Planning — No third-party names in planning text").

**Labels:** `work-unit`; `ready` / `blocked` (later `in-progress`, `status:blocked-failure`);
`type:feature`, `type:fix`, `type:infra`, `type:docs`, `type:content` (tables, rules, art data);
`area:` app, render, physics, sim, input, audio, assets, tablegen, assetgen, script, tables, ci,
release, process; `phase:NN-name`; `tier:T1` / `tier:T2` (expected; T3 is never planned); and
`review:skip` / `review:sanity` / `review:full` (`docs/PROCESS.md` P6). During an initial-usable-build
milestone (M1 `#79`), label boundary, contract, schema, timing/concurrency, `very-high`
blast-radius, R11 security and automated-gate units `review:sanity`, routine vertical-slice/content
units `review:skip`, and include a cohesive full-review unit (`review:full`, e.g. `#153` KP-064)
once the end-to-end build passes its milestone gate before flipping to post-usable per-PR review.
**Milestones** are tracking issues (label `tracking`): #79 M1 — Play experience, #80 M2 — Backlog,
or a new one whose body states the goal and scope (no Plan ID line) and which closes with its
last sub-issue.

**Suggested phase order** (adapt to the requirements): responsiveness and wiring fixes →
contracts (`pin_core` CRs, new schemas) → rendering capabilities → the machine (scene, cameras,
parts, cabinet, art) → tables → automated gates → cohesive full review pass → acceptance material →
backlog.

## 3. Check the breakdown

1. Every REQ ID in SPEC (except `[WITHDRAWN]`) appears in at least one unit:
   `python3 -I scripts/req_coverage.py --plan-file <draft>` on the draft, and
   `python3 -I scripts/req_coverage.py` against the live issues once they exist.
2. The dependency graph is acyclic and every dependency comes earlier in plan order.
3. Owned paths of units that can run at the same time do not overlap.
4. Every contract in PROCESS.md P3 that a unit changes or adds has an owning unit that comes
   before its first consumer.

## 4. Create the issues

0. List the existing `work-unit` issues, open and closed (label search, explicit paging), and
   match each drafted unit to one by its `Plan ID`. Leave unaffected units alone. Edit a changed
   unit in place (`issue_write`, method `update`: body, labels) only while it is open. Never edit
   a closed (completed) unit to carry new work: `work-issue` stops on a closed issue. When a SPEC
   change affects a completed unit, create a new follow-up unit instead (next free `KP-NNN`) that
   depends on the completed one and references it. Create only units with no match, using the
   next free `KP-NNN` numbers. Close units dropped from the breakdown as "not planned"
   (`issue_write`, `state_reason` `not_planned`), with a comment naming what replaced them.
1. Ensure every label and tracking issue exists before the first unit that uses it is created
   (a missing label the session cannot create is a `[P8 setting]` line).
2. Open the new issues in plan order (`issue_write`, method `create`), so every "Depends on" entry
   names an existing issue number. Pace the calls (about one per second) to stay under GitHub's
   content-creation limits. Attach each one to its tracking issue (`sub_issue_write`, method
   `add`, with the new issue's numeric `id`, not its number).
3. The body's "Depends on" list is the only dependency record (no "blocked by" links). Label a
   unit `ready` only when every dependency is complete under the merged-PR rule (PROCESS.md P6
   step 5, `work-issue` §1 step 2); label the rest `blocked`. An open dependency, or one closed as
   not planned without a qualifying replacement, is not complete.
4. Run `python3 -I scripts/req_coverage.py` and fix any gap with an issue edit.
5. Nothing is committed for the plan itself. Record any decisions made while planning in
   `docs/DECISIONS.md` through a normal PR. Then: "Start work on issue X" (`work-issue`) or
   `run-plan`.
