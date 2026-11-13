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
| 2 | 1.3 | Implementer | Opus 5.5 | 1 | done | all own tests pass; coverage pin_core 99 %, pin_render 100 %, pin_app 92 % | CR-001 `pin_core::spsc` (additive) |
| 2 | 1.3 | Independent Tester | Fable 5.2 | 1 | done | 60 spec tests | |
| 2 | 1.3 | Reviewer | Fable 5.2 | 1 | **pass** | gate 1–8 pass; SPSC ordering verified | 7 non-blocking; `pin_app/src/lib.rs` accepted |
| 2 | 2.1 | Independent Tester | Opus 5.5 | 1 | done | 66 spec tests pass | |
| 2 | 4.2 | Independent Tester | Fable 5.2 | 1 | done | 135 pass, 2 fail, 6 hang | 4 defects sent to Implementer |
| 2 | 2.1 | Implementer | Fable 5.2 | 1 | done | 55 own tests; coverage 97.5 %; PLAN scenario passes | |
| 2 | 2.1 | Reviewer | Opus 5.5 | 1 | **fail** | gate 1–7 pass | B1: `GuideCurve::cast_sphere` false hits on separating/parallel balls (~150 µs each); fix 1 sent with N1, N4–N7 |
| 2 | 4.2 | Implementer | Opus 5.5 | 1 | done | 158 spec + own tests; coverage 98 %; zero allocs incl. Lua VM | 4 tester defects fixed; reviewer running |
| 2 | 4.2 | Reviewer | Fable 5.2 | 1 | **fail** | gate 1–7 pass | B1 C-level loops hang, B2 `<close>` unhooked, B3 `__pairs` leaks backing table, B4 budget × size stalls, B5 save-state escapes memory cap, B6 `pairs` order non-deterministic |
| 2 | 1.2 | Implementer + Tester | Opus 5.5 / Fable 5.2 | fix 1 | done | 38 spec tests; actionlint clean | CI pinned to 1.97.0 |
| 2 | 1.2 | Reviewer | Fable 5.2 | 2 | **pass** | run 43149748681 fully green (12 jobs, 3 OS, 4m14s); core-tests display-free; SDL from source builds in gpu-tests | N6 `--locked` deferred to end of wave 2 |
| 2 | 2.1 | Implementer | Fable 5.2 | fix 1 | done | 127 tests; coverage 97.5 % | B1 fixed: GuideCast { Contact, Advance } |
| 2 | 2.1 | Reviewer | Opus 5.5 | 2 | **pass** | gate 1–7 pass; 20k fuzz: 0 false/missed contacts; outside-arc cast 150 µs → 1.65 µs | N1 sink ratchet, N2 closed loops: hardening round requested; N3, N4 → 2.2 |
| 2 | 2.1 | Implementer | Fable 5.2 | hardening | done | 130 tests; coverage 97.9 % | fixed sliding floor; periodic closed guides; proven curvature bound |
| 2 | 2.1 | Reviewer | Opus 5.5 | 3 | **pass** | gate green; probes: no ratchet (−10 µm max), closed loops glide, 0 deep penetrations in 30 754 adversarial casts | near-closed loops (ends within ~1e-8 m) bypass exact-equality check → 4.1 |
| 2 | 4.2 | Implementer | Opus 5.5 | fix 1 | done | 252 tests incl. 42 stall cases; coverage 98.1 %; zero allocs | patched vendored Lua (10 KHZ patches); B1–B6 fixed; faulting callback drops its own commands |
| 2 | 4.2 | Independent Tester | Fable 5.2 | 2 | done | hardening + cross-process specs | pointer-keyed pairs ruling applied |
| 2 | 4.2 | Reviewer | Fable 5.2 | 2 | **fail** | gates 1–7 pass; CI green on 3 OS with patched Lua; B1–B6 confirmed fixed | B7 uncharged pack/unpack/%q loops (26 s), B8 rolled-back hold release strands coil, B9 cheap-call/expensive-run ops (up to 557 ms at 1M) |
| 2 | 4.2 | Independent Tester | Fable 5.2 | 3 | done | 225 pass / 3 fail (round-2 items) / 0 hang | |
| 2 | 4.2 | Implementer | Opus 5.5 | fix 2 | done | 282 tests; coverage 98.1 %; worst tick 4.8 ms at 100k budget | pack removed; per-C-call charge; rollback keeps hold releases |
| 2 | 4.2 | Reviewer | Fable 5.2 | 3 | **fail** | gates 1–7 pass; CI green; B7–B9 confirmed fixed | B10 next() over emptied array 1 s; B11 GC thrash 30–45 ms/tick, no fault; B12 string.rep loop; B13 buffers under cap 20–40 ms; B14 large rehash 16 ms |
| 2 | 4.2 | Independent Tester | Fable 5.2 | 4 | done | 242 spec tests pass | B10–B13 cases; engine-config fixture run |
| 2 | 4.2 | Implementer | Opus 5.5 | fix 3 (last) | done | 301 tests; coverage 98.1 %; worst tick 4.68 ms under engine config (163 scripts) | central charging in lauxlib/ltable/lgc; `LuaConfig::engine()` |
| 2 | 4.2 | Reviewer | Fable 5.2 | 4 | **pass** | gates 1–8 pass; 174 attack scripts under engine config, worst 5.7 ms (target 8 ms); CI green | N11 heavy-GC scripts fault under 2 MiB/100k (documented), N12 margin machine-dependent, N13 corpus kept in tests/attack_corpus |

**Wave 2 gate: green.** CI run 44 (https://github.com/donfuldauer/kilohertz-pinball/actions/runs/43230217782, commit 8d7f663) passed every job on Windows MSVC, Linux and macOS aarch64, which also re-validates Wave 1 cross-OS. Replay-hash and tick-benchmark parts of the wave gate start once 2.5 exists. Subphases: 1.2, 1.3, 2.1, 3.1, 4.2 all passed review.
| 3 | 3.2 | Implementer | Opus 5.5 | 1 | done | fmt/clippy clean; 63 own tests; coverage 90.5 % (own) | 2 tester-found defects fixed in-run |
| 3 | 3.2 | Independent Tester | Fable 5.2 | 1 | done | 110 spec tests pass; coverage 99.27 % | |
| 3 | 3.2 | Reviewer | Fable 5.2 | 1 | **fail** | gates 1–7 pass | B1 phantom plunger releases from deadzone-edge noise (30 in 3 s); N1–N4, N6, N8 folded into fix 1; N5 → 4.4/6.0 |
| 3 | 3.3 | Implementer | Opus 5.5 | 1 | done | fmt/clippy clean; coverage 97.1 %; render 4.7 µs | |
| 3 | 3.3 | Independent Tester | Fable 5.2 | 1 | done | 82 spec tests pass | Ogg tail-trim defect found and fixed in-run |
| 3 | 3.3 | Reviewer | Fable 5.2 | 1 | **fail** | gates 1–7 pass | B1 crafted .ogg aborts process (lewton unbounded with_capacity); B2 lying sample rate defeats 10-min cap (567 MB from 185 KB); N1, N4, N7 folded into fix 1; N3 → loader |
| 3 | 1.4 | Independent Tester | Fable 5.2 | 1 | done | 88 spec tests pass (GPU on lavapipe) | read free-form thread API names from impl for one file |
| 3 | 2.2 | Independent Tester | Opus 5.5 | 1 | done | 41/44 pass | defect: impacts starting inside contact skin swallowed → implementer |
| 3 | 4.1 | Independent Tester | Fable 5.2 | 1 | done | 121/126 pass | D1 load_gltf panics inside gltf crate on bad refs → implementer |
| 3 | 4.5 | Independent Tester | Fable 5.2 | 1 | done | 79 spec tests pass | release ≤ 8 ms timing not measured (disk full) → reviewer |
| 3 | 3.2 | Implementer | Opus 5.5 | fix 1 | done | coverage 99.31 % | B1, N1–N4, N6, N8 |
| 3 | 3.2 | Reviewer | Fable 5.2 | 2 | **fail** | gates 1–7 pass; B1, N1–N6 confirmed fixed | B2 pushing plunger in > 1 s disables launches permanently (downward re-acquire without recovery) → fix 2 |
| 3 | 3.3 | Implementer | Opus 5.5 | fix 1 | done | coverage 97.82 % | vendored lewton with KHZ bounds; source-rate bounds; streaming resample; chained → Unsupported |
| 3 | 2.2 | Implementer | Fable 5.2 | 1 | done | coverage 97.94 %; 0 allocs; 10-run determinism | tester defect (skin impacts) fixed; spec 54/54 |
| 3 | 4.1 | Implementer | Opus 5.5 | 1 | done | coverage 96.99 %; spec 124/124 | tester D1 (gltf panics) fixed with check.rs + catch_unwind |
| 3 | 1.4 | Implementer | Opus 5.5 | 1 | done | coverage render 94.5 %, app 84.9 %; drift 0.052 % | CR-002 SimStep |
| 3 | 3.3 | Reviewer | Fable 5.2 | 2 | **pass** | gates 1–8 pass; coverage 97.22 %; render 4.6 µs; 54 valid decodes bit-identical to round 1 | B1, B2, N1, N4 verified; lewton patch audited (no unbounded allocation left); N10 lying-granule reservation (address space only), N11–N13 → HUMAN_QUEUE |
| 3 | 3.2 | Reviewer | Fable 5.2 | — | — | — | (see fix 2 above; re-review pending) |
| 3 | 1.4 | Reviewer | Fable 5.2 | 1 | **fail** | gates 1–3, 5–7 pass; drift 0.0025 %; visual checklist pass | B1 pin_render standalone coverage 58.25 % (surfaces/blit only exercised from pin_app) — no waiver; N1–N7 folded into fix 1 |
| 3 | 4.1 | Reviewer | Fable 5.2 | 1 | **fail** | gates 1–7 pass; 196 tests; coverage 96.99 % | B1 lying compressed_size bypasses ratio cap; B2 no glTF aggregate budget (91 KB → 415 MB); B3 non-finite world transforms accepted; N1, N3–N6, N9 folded into fix 1 |
| 3 | 2.2 | Reviewer | Opus 5.5 | 1 | **fail** | gates pass; coverage 97.87 %; 4–5 µs/step | B1 rising curved guides stall (~10× slow); B2 C2 bit-identity breaks with a moving steel ball nearby; mesh-floor normal glitches raised to required; B3 flush-cuboid seam hop downgraded (tables use meshes); world/ ownership amendment for fix 1 |
| 3 | 3.2 | Implementer | Opus 5.5 | fix 2 | done | coverage 99.28 % | B2 fixed; never_stuck proptest |
| 3 | 3.2 | Reviewer | Fable 5.2 | 3 | **fail** | gates 1–7 pass; 16 000 noise-free histories: 0 violations | B3 ARM_REST_MS loses launches under sensor noise (up to 85 % at ±0.004); N-f, N-g folded into fix 3 (last) |
| 3 | 4.5 | Implementer | Opus 5.5 | 1 | done | coverage wasm 95.7–98.1 %, SDK 97.4 %; spec 79/79 | two 4.2 tests amended (ownership amendment) |
| 3 | 4.5 | Reviewer | Fable 5.2 | 1 | **fail** | gates 1–7 pass; release faulting tick 0.40–0.67 ms (limit 8) | B1 no module-size/function-count cap (8 MB → 62 s, 5.7 GB); N1–N5 folded into fix 1 |
| 3 | 4.1 | Implementer | Opus 5.5 | fix 1 | done | 211 tests; coverage 96.89 % | live-ratio inflate (flate2), data-range checks, GltfLimits budgets, finite checks |
| 3 | 4.1 | Reviewer | Fable 5.2 | 2 | **pass** | gates 1–8 pass; coverage 96.85 % | B1–B3 verified by probe; N-a Zip64 count pre-check bypass, N-b aggregate ratio under floor, N-c decoder peak, N-d buffer read size → HUMAN_QUEUE |
| 3 | 1.4 | Implementer | Opus 5.5 | fix 1 | done | pin_render standalone 95.65 % | gpu_window + blit readback; configure race found and fixed (gpu_lock + try_configure); N1–N7 |
| 3 | 1.4 | Reviewer | Fable 5.2 | 2 | **pass** | gates 1–8 pass; gpu_window 12/12, orchestrator_window 12/12; pin_render 95.50 %, pin_app 92.49 % | N-a silent unconfigurable surface, N-b detached presenter vs window drop, N-c tiny-circle gaps → HUMAN_QUEUE |
| 3 | 3.2 | Implementer | Opus 5.5 | fix 3 (last) | done | 0 lost / 0 doubled over 28 800 noisy launches | arming persists; noise-aware speed estimator |
| 3 | 3.2 | Reviewer | Fable 5.2 | 4 | **fail** | gates 1–7 pass; invariant fuzz 0 violations at rest 0 (with/without noise) | B4 estimator biased low on noise-free curved strokes (−6 % to −44 %); N-k, N-l |
| 3 | 3.2 | Implementer (other model) | Fable 5.2 | 1 | running | — | fresh brief + failure history (PLAN §2.6 step 2); review by Opus |
| 3 | 2.2 | Implementer | Fable 5.2 | fix 1 | done | 249 tests; coverage 97.59 % | rising-guide reach bound; per-ball clocks; f64 face distance; resting seating |
| 3 | 2.2 | Reviewer | Opus 5.5 | 2 | **fail** | gates 1–7 pass; round-1 B1, B2, mesh glitches, drift verified fixed | B4 tunnelling through tight/steep rising guides (non-global closest point, pre-existing since 2.1) → fix 2 |
| 3 | 3.2 | Implementer (other model) | Fable 5.2 | 1 | done | 293 pin_input tests; coverage 98.78 % | B4 exact noise-free (all shapes 0.000 %); N-k, N-l fixed; offset never_stuck 80 000 histories clean |
| 3 | 2.2 | Implementer | Fable 5.2 | fix 2 | done | 251 tests; coverage 97.78 % | global closest point for sloped guides; tunnelling regression tests |
| 3 | 4.5 | Implementer | Opus 5.5 | fix 1 | done | 436 tests | module size/count pre-scan, max_wasm_stack, N2–N5 |
| 3 | 4.5 | Reviewer | Fable 5.2 | 2 | **fail** | gates 1–7 pass; faulting tick ≤ 0.62 ms | B1 partly fixed: modules within all caps still compile up to 172 s / 2.4 GB (locals × nesting, br_table, types, element items) → fix 2 with per-construct caps, target < 1 s / 256 MB |
| 3 | 2.2 | Reviewer | Opus 5.5 | 3 | **fail** | gates 1–7 pass; B4 tunnelling verified fixed (all shapes, 0 penetrations) | B5 sloped-guide cost (4 balls up to 4.3 ms/step) and helix stall (4–9 % of v·dt) → fix 3 (last) |
| 3 | 3.2 | Reviewer | Opus 5.5 | 5 (after handover) | **fail → pass by Orchestrator ruling** | gates 1–7 pass; 293 tests; coverage 98.72 %; 0 lost/doubled/phantom in 28 800 × 10 noise models and 384 000 histories | B5 noisy curved-stroke accuracy 5–10 % low ruled non-gating (DECISIONS "3.2 final ruling"; HUMAN_QUEUE asks a person to confirm) |
| 3 | 2.2 | Implementer | Fable 5.2 | fix 3 (last) | done | 253 tests; coverage 97.81 %; sloped query 0.7–0.9 µs, 4 balls 20–22 µs/step | sloped-guide acceptance rule (helices, tight bends rejected); exact plan-view closest point |
| 3 | 2.2 | Reviewer | Opus 5.5 | 4 | **fail** | gates 1–7 pass; sliding beside sloped walls 7–9 µs/step, 0 slow ticks; spikes = machine noise | B6 top/bottom edge region of sloped guides takes the slow path (0.1–1 ms/step, worst 15 ms, up to 90 % distance lost); NB1 bend limit docs |
| 3 | 2.2 | Implementer (other model) | Opus 5.5 | 1 | running | — | fresh brief + failure history (PLAN §2.6 step 2); review by Fable |
| 3 | 4.5 | Implementer | Opus 5.5 | fix 2 | done | all pin_script targets pass; worst compile at caps 725 ms / 60 MB | operator scan with nesting/merge/branch/pressure budgets; caps lowered; N7 |
| 3 | 2.2 | Implementer (other model) | Opus 5.5 | 1 | done | coverage 97.77 %; edge regions 2–7 µs/step mean, ≤ 172 µs worst (4 balls) | wall reach (Federer), edge Newton closest point, rolling-ball cast bound; vertical-bend acceptance rule; NB1 docs |
| 3 | 4.5 | Reviewer | Fable 5.2 | 3 | **fail** | gates 1–7 pass; 17 at-cap cases ≤ 0.68 s | B1 still open: call_indirect / table.* unbudgeted (640 KB → 76.5 s, 716 MB); N9 realistic headroom (dev builds 74 % merge) → fix 3 (last), systematic operator sweep required |
