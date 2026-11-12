# Run Log

Maintained by the Orchestrator (PLAN.md §2.6). One row per subphase attempt.

| Wave | Subphase | Role | Model | Attempt | Status | Gate results | Notes |
|---|---|---|---|---|---|---|---|
| 0 | 0.1 | Implementer | Opus 5.5 | 1 | done | fmt/clippy clean; all spikes pass (Linux local) | commit a417638 |
| 0 | 0.1 | Independent Tester | Fable 5.2 | 1 | done | 32/32 spec tests pass | commit b9d9bc3 |
| 0 | 0.1 | Reviewer | Fable 5.2 | 1 | **pass** | gate 1–7 pass; CI run 43140608468 green on ubuntu/windows/macos | 10 non-blocking findings; NB3/NB4 folded into 1.2 brief, NB5–NB9 deferred |

**Wave 0 gate: green** (single subphase; CI green on all three OSes).
| 1 | 1.1 | Independent Tester | Fable 5.2 | 1 | done | 117 spec tests | commit 83ab239 |
| 1 | 1.1 | Implementer | Opus 5.5 | 1 | done | fmt/clippy clean; 177 tests; coverage 99.38 % | commits f21f545, 0bceb19 |
| 1 | 1.1 | Reviewer | Fable 5.2 | 1 | **pass** | gate 1–8 pass; triple-buffer protocol verified | 7 non-blocking findings |
| 1 | 1.1 | Implementer | Opus 5.5 | fix 1 | done | 184 tests; coverage 99.33 %; Orchestrator re-ran fmt/clippy/tests | N1, N2, N3, N6 fixed; N4, N5 → 4.1, N7 → 2.4 |

**Wave 1 gate: green on Linux.** Cross-OS workspace tests start with the CI matrix in Subphase 1.2 (Wave 2); Wave 1 is re-validated there.
| 2 | 1.2 | Independent Tester | Fable 5.2 | 1 | done | 34 spec tests | commit c0c4500 |
| 2 | 1.2 | Implementer | Opus 5.5 | 1 | done | actionlint clean; Linux commands pass | workflows in cf246e2 |
| 2 | 1.2 | Reviewer | Fable 5.2 | 1 | **fail** | B1: `@stable` (1.99.1) clippy lint breaks pin_core | spikes CI green on 3 OS; ci machinery works; other red jobs are wave-2 WIP |
| 2 | 1.2 | Implementer + Tester | Opus 5.5 / Fable 5.2 | fix 1 | running | — | pin CI to 1.97.0; N2, N4, N5, N7, N3 |
| 2 | 3.1 | Implementer | Opus 5.5 | 1 | done | 26 own tests; coverage 100 % | |
| 2 | 3.1 | Independent Tester | Fable 5.2 | 1 | done | 71 spec tests | |
| 2 | 3.1 | Reviewer | Fable 5.2 | 1 | **pass** | gate 1–8 pass; 100 % lines; alloc guard real | 4 non-blocking: N1 → 2.5 brief (callers clear `out` before `update`), N2 doc, N4 compile-time assert, N3 none |
