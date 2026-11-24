Closes #<!-- issue number -->

## Work item

<!-- issue title, plan ID (KP-NNN), phase label, tracking issue (#79 M1 or #80 M2),
     whether this is a gate issue (last open issue of its phase or tracking issue, or
     `review:gate`; docs/PROCESS.md P6) with a link to its gate review PR, or "routine (no code
     review)";
     the branch, if the session could push only to an assigned branch -->

## Requirements satisfied

| SPEC section / REQ ID | Deliverable | Proven by (test, gate or evidence) |
|---|---|---|
|  |  |  |

## Gate results (docs/PROCESS.md P5 unit gate)

- Format / lint (`cargo fmt --all --check`; `cargo clippy --all-targets -- -D warnings`, with `--features gpu-tests` for render crates):
- Tests (run / passed / failed / ignored, per suite):
- Coverage per crate (floor: 85 % core crates, 60 % `pin_render` / `pin_audio` / `pin_app`):
- Property tests and bounded random-bytes runs (parsers touched):
- Sim thread: allocation guard; replay hash ×10:
- Gates A1 / A2 / A3 (when touched; else "not touched"):
- Owned paths (path check; shared files appended only):
- Work-verifier: pass @ <sha>

## Visual and windowed evidence (docs/PROCESS.md R12; render, presentation, input or timing work)

- Analytic pixel assertions:
- Regression snapshots (stored after the analytic checks):
- Screenshots and the visual checklist (images attached, each item's result):
- Windowed app run (Xvfb + lavapipe):

## Decisions, contract changes, queue lines, new issues

- DECISIONS.md:
- contracts/CHANGE_REQUESTS.md:
- HUMAN_QUEUE.md (P8 lines only, else "none"):
- New `work-unit` issues for discovered or deferred work:

## Model routing

<!-- harness and tiers used; every T3 / Fable 5.2 use with its P4 evidence (need, why T2 was not
     enough, allowance: the owner's answer and time on Claude Code or plan/quota on Gemini /
     Antigravity); [Fable wanted] / [T3 wanted] notes -->
