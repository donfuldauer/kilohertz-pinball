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
| 3 | 2.2 | Reviewer (final) | Fable 5.2 | after handover | **pass** (B7 → 2.1 fix task) | gates 1–7 pass; 259 tests; coverage 97.72 %; B6 verified on 19 shapes (0 crossings, ≤ 9.2 µm); all earlier fixes re-verified | B7 level-guide open-end reach (pre-existing 2.1) opened as a 2.1 fix task; N1–N5 non-blocking |
| 3 | 2.1 (fix task B7) | Implementer | Fable 5.2 | 1 | done | 270 pin_physics + 211 pin_assets tests; coverage 97.71 %; no 2.1 assertion re-baselined | true open-end reach + tangent-ball table; solver penetration 2.5 mm → 6.8 µm |
| 3 | 4.5 | Implementer | Opus 5.5 | fix 3 (last) | done | all pin_script targets pass; release operator sweep (804 cases) ≤ 0.19 s / 55 MB; combined worst 0.42 s / 64 MB | per-operator cost classes, operand-stack liveness, loop work; SIMD/tail-call/extended-const off; nesting 512 |
| 3 | 4.5 | Reviewer | Fable 5.2 | 4 | **pass** | gates 1–8 pass; 444 tests; ~1 400 random + 29 hand-built at-limit modules: worst 0.39 s / 53 MB; faulting tick ≤ 0.40 ms | N9 dev builds exhaust merge budget (use opt-level=1), N10 2.4× margin, N11 SIMD disabled, N8 sim-thread stack ≥ 2 MiB → HUMAN_QUEUE |
| 3 | 2.1 (fix task B7) | Reviewer | Opus 5.5 | 1 | **fail** | gates 1–7 pass; 0 tangent-ball violations / 686 tables; 37 622 casts worst gap +4.93 µm; bit-identical on non-end-limited guides | narrow openings drop the table → crawl along the whole wall (up to 10×) → fix 2 (adaptive stride) |
| 3 | 2.1 (fix task B7) | Implementer | Fable 5.2 | fix 2 | done | 271 + 211 tests; coverage 97.74 % | adaptive tangent-ball table resolution (4–64 per segment); narrow rings slide at full speed |
| 3 | 2.1 (fix task B7) | Reviewer | Opus 5.5 | 2 | **fail** | gates 1–7 pass; correctness clean; narrow rings ≥ 97.9 % v·dt | construction cost quadratic (300 seg, 0.6 mm gap: 1 075 ms) → fix 3 (last); nearly-closed level guides to be rejected |
| 3 | 2.1 (fix task B7) | Implementer | Fable 5.2 | fix 3 (last) | done | 274 + 211 tests; build ≤ 36.8 ms at 2 000 segments (was up to 1 075 ms at 300) | level_reaches bounding-sphere pruning; resolution-free tangent-ball table |
| 3 | 2.1 (fix task B7) | Reviewer | Opus 5.5 | 3 | **fail** | gates 1–7 pass; level construction ≤ 20 ms adversarial; sliding, cost, bit identity pass | J/T tip tangent-ball regression (mini ball 114 µm in); end pointing between samples still overestimates (−388 µm, pre-existing); sloped construction 2.6–4.5 s at 2 000 seg (2.2 code) |
| 3 | 2.1 (fix task B7) + 2.2 sloped construction | Implementer (other model) | Opus 5.5 | 1 | running | — | fresh brief + failure history (PLAN §2.6 step 2); review by Fable |
| 3 | 2.1 (fix task B7) + 2.2 sloped construction | Implementer (other model) | Opus 5.5 | 1 | done | 278 + 211 tests; coverage 94.08 %; 0 brute-force violations on 337 shapes; sloped build ≤ 42.6 ms at 2 000 seg (was 4.5 s) | proven between-sample bounds (curve_bounds.rs); near-linear sloped tests (sloped_bounds.rs); review by Fable |
| 3 | 2.1 (fix task B7) + 2.2 sloped construction | Reviewer | Fable 5.2 | other-model 1 | **fail** | gates 1–7 pass; 0 tangent-ball violations on 564 new shapes; 62 134 casts worst +4.49 µm; solver −6.54 µm; sloped build 23.8–53.6 ms (was 2.3–4.8 s) | B1 sliding crawl (5 % v·dt, cast 3×) in ball-sized lanes of end-limited spirals; B2 level coils build up to 279 ms (> 50 ms) |
| 3 | 2.1 (fix task B7) + 2.2 sloped construction | Orchestrator | — | — | **blocked** (D29) | correctness blockers 1–3 closed; perf targets B1/B2 missed | other-model attempt kept on the branch (safest state); see DECISIONS "Orchestrator — 2.1 B7 blocked" and HUMAN_QUEUE |
| 3 | Wave 3 gate | Orchestrator | — | CI a4c977b | **fail** | fmt, clippy ×3, Linux core, Windows GPU green | Windows pin_assets n4 (CRLF → .gitattributes); macOS spec_wasm_sandbox budget at load (4.5 fix); lavapipe spec_debug_render SIGSEGV + Metal orchestrator_window 0/30 presents (1.4 fix) |
| 3 | 1.4 (Wave 3 gate fix) | Implementer | Opus 5.5 | 1 | done | lavapipe suite + 20 reruns green locally; pin_render coverage 95.21 % | Metal: wgpu Occluded until the window is visible → tests pump events + warm-up, SurfaceStatus::Occluded; lavapipe: process-wide lock around device create/teardown (crash not reproduced locally) |
| 3 | 1.4 (Wave 3 gate fix) | Reviewer | Fable 5.2 | 1 | **pass** | 7/7 local lavapipe full runs, 40× spec_debug_render loop; coverage 94.6 %; CI run 113: gpu-tests green on lavapipe, Metal, WARP | lavapipe root cause is a hypothesis (N1): watch the next CI runs; N2–N6 non-blocking |
| 3 | 4.5 (Wave 3 gate fix) | Implementer | Opus 5.5 | 1 | done | pin_script all green; coverage 97.58 % | root cause: wasmtime CoW memory init only on Linux (memfd); macOS/Windows run metered init (1 fuel/byte) from the start budget → memory_init_cow(false) everywhere + exact init_fuel allowance on top of the budget |
| 3 | 4.5 (Wave 3 gate fix) | Reviewer | Fable 5.2 | 1 | **pass** | 470 pin_script tests; 700/700 random modules exact; never over-grants fuel; coverage 97.56 % | N1/N2 bounded under-counts (empty expr elem segments, ref-typed globals), N3 docs wording → HUMAN_QUEUE |
| 3 | **Wave 3 gate** | Orchestrator | — | CI run 115 (a07ad35) | **green** | all 12 jobs: fmt; clippy, core-tests on Linux/macOS/Windows; gpu-tests on lavapipe/Metal/WARP; coverage | https://github.com/donfuldauer/kilohertz-pinball/actions/runs/43377831662 — lavapipe green in runs 113 and 115. Passed: 1.4, 2.2, 3.2, 3.3, 4.1, 4.5. Open: 2.1 fix task B7 blocked (performance only; correct code on the branch). Replay hash and tick benchmark n/a until 2.5 |
| 3→4 | Human rulings (queue §1) | Orchestrator | — | — | recorded | all six recommendations approved | DECISIONS "Human rulings — end of Wave 3" |
| 3→4 | 2.1-B1 (lane sliding) | Implementer | Opus 5.5 | 1 | running | — | review by Fable; B2 accepted as known limit |
| 3→4 | 4.1-H (hardening + guide lints) | Implementer | Opus 5.5 | 1 | running | — | review by Fable |
| 3→4 | 3.3-H (decode hardening, audio budget) | Implementer | Opus 5.5 | 1 | running | — | review by Fable |
| 3→4 | 3.3-H | Implementer | Opus 5.5 | 1 | done | pin_audio all green; coverage 97.28 %; decode outputs bit-identical | reservation ≤ 64 Ki frames then on-demand growth; DecodeBudget (512 MiB default, exact, atomic); wiring → 2.5/6.0 |
| 3→4 | 4.1-H | Implementer | Opus 5.5 | 1 | done | 235/236 pin_assets tests; coverage 96.44 %; hostile inputs rejected ≤ 10 ms, ~0 MiB extra | 1 spec test (uneven spacing, cusp guide) predates the guide-lint ruling → Tester amendment |
| 3→4 | 3.3-H | Reviewer | Fable 5.2 | 1 | **pass** | 132 pin_audio tests; coverage 97.17 % (decode.rs 98.68 %); 132 hostile cases bounded; 36 448-case budget fuzz exact; 300 concurrent rounds, 0 overcommits; bit-identical to pre-fix | R1–R9 non-blocking → HUMAN_QUEUE |
| 3→4 | 4.1-H | Tester (spec amendment) | Fable 5.2 | 1 | done | spec_manifest 39 tests; pin_assets all green | uneven-spacing test re-based on a cusp-free guide; 5 new spec tests for lints (b)–(d); notes: (c) ignores layer restrictions, (d) can fire 5–14 mm from the wall, (b) fires on coarse hand-authored polygons |
| 3→4 | 4.1-H | Reviewer | Fable 5.2 | 1 | **fail** | gates 1–7 pass (241 tests, coverage 96.44 %); N-b/N-c/N-d as specified hold | B1 Zip64 tail-window off-by-one (1.6 s, 569 MiB); B2 zip32 prepended-data searches (signature flood 128 MiB; candidate fallback super-linear, 18 s at 40); B3 data: URI buffers peak 5× (1.1 GiB); B4 lint (d) wrong advice on cusps → fix 1 |
| 3→4 | 4.1-H | Implementer | Opus 5.5 | fix 1 | done | pin_assets all green; coverage 96.57 %; every reviewer probe ≤ 19 ms, ≤ 5 MiB | single end record at EOF + ArchiveOffset::Known(0); glTF JSON cap 16 MiB; image header alloc 4 MiB + plausibility; lint (d) split (gap advice vs doubles-back) |
| 3→4 | 4.1-H | Reviewer | Fable 5.2 | 2 | **fail** | 253 tests; coverage 96.48 %; B1–B4 closed (all archives ≤ 15 ms, ~0 MiB; legit Info-ZIP/Python/jar/Go zips load); refusing prepended data accepted | B5 self-touch lint O(n²) on y-running guides (2000 pts 41 s) → fix 2 |
| 3→4 | 4.1-H | Implementer | Opus 5.5 | fix 2 | done | 255 tests; coverage 96.11 % | B5: grid-based far-pair search, linear (2000-pt vertical 41 s → 26 ms); 4 096 through-point cap per spline (error) |
| 3→4 | 4.1-H | Reviewer | Fable 5.2 | 3 | **pass** | 255 tests; coverage 96.14 %; B5 closed (4 096-pt vertical 70 ms; dense serpentines/spirals ≤ 0.28 s); 4 096-point cap accepted | residual: km-scale tail stretches grid cells (≤ 2.4 s/spline) → HUMAN_QUEUE |
| 3→4 | 2.1-B1 | Implementer | Opus 5.5 | 1 | done | B1 spiral 0/191 slow ticks; pitch band 72 cases 0 slow; cast 0.05–0.38× 329c843; 1.5 M lane checks 0 violations; coverage 94.25 % | lane bound (R, D) + reach not cut by tolerances; 4 guides build 1.11–1.14× (≤ +3.7 ms) — accepted by Orchestrator ruling; review by Fable |
| 3→4 | 2.1-B1 | Reviewer | Fable 5.2 | 1 | **pass** | 284 tests; coverage 94.25 %; 5.49 M lane checks 0 violations; B1 0/191 slow; band 0/19 755 slow; bit-identical on 239/239 unchanged shapes | residual N1 (budget-exhausted coils) / N2 (coarse sampling unclassified) → HUMAN_QUEUE |
| 3→4 | fix tasks | Orchestrator | — | — | **CI green** | run 132 attempt 2 (5a318c4), Linux/macOS/Windows: fmt, clippy, core-tests, gpu-tests (lavapipe/Metal/WARP), coverage | 2.1-B1, 3.3-H, 4.1-H confirmed cross-OS; runs 130–132 attempt 1 failed only by macOS jobs cancelled without a runner (no test failures); https://github.com/donfuldauer/kilohertz-pinball/actions/runs/43479638307 |
| 4 | — | Orchestrator | — | — | started | — | Wave 4 plan: seam task 2.2-S first for 2.3/2.4/2.7; batch A 2.2-S, 3.4, 4.3; batch B 4.6, 5.1, then 2.3, 2.4, 2.7 (DECISIONS "Orchestrator — Wave 4 brief decisions") |
| 4 | 2.2-S | Implementer | Opus 5.5 | 1 | running | — | moving-body + force-context seams; review by Fable |
| 4 | 3.4 | Implementer | Opus 5.5 | 1 | running | — | |
| 4 | 3.4 | Independent Tester | Fable 5.2 | 1 | running | — | |
| 4 | 4.3 | Implementer | Opus 5.5 | 1 | running | — | |
| 4 | 4.3 | Independent Tester | Fable 5.2 | 1 | running | — | |
| 4 | 4.6 | Implementer | Opus 5.5 | 1 | running | — | |
| 4 | 4.6 | Independent Tester | Fable 5.2 | 1 | running | — | |
| 4 | 3.4 | Independent Tester | Fable 5.2 | 1 | done | 84 spec tests pass; clippy clean on spec files | no implementation defects; tests beyond the brief pin documented choices (rumble envelope, SSF defaults, chunk invariance) — flagged for the Reviewer |
| 4 | 4.6 | Independent Tester | Fable 5.2 | 1 | done | 46 spec tests pass | no defects; V5 scenes read as out of kit scope (AssetKind has no scene kind) — Reviewer to confirm; clippy blocked by in-progress edits in other crates at the time |
| 4 | 4.3 | Independent Tester | Fable 5.2 | 1 | done | 50/52 spec tests pass | D1 tiny ramp entry_width emits a degenerate layer-transition zone (invalid bundle) → sent to the Implementer |
| 4 | 4.6 | Implementer | Opus 5.5 | 1 | done | 52 own tests; coverage 96.75 %; all spec tests pass; clippy clean (--no-deps) | WAV-only sounds; no scene kind (5.3 owns scenes); tester spec_4_6_fonts clippy nit sent back |
| 4 | 4.6 | Reviewer | Fable 5.2 | 1 | running | — | |
| 4 | 4.3 | Implementer | Opus 5.5 | 1 | done | clippy clean (pin_tablegen + pin_assets); coverage pin_tablegen 95.7 %, pin_assets 96.2 % | D1 fixed (0.1 mm minimum size, zone check); 4.1-H follow-ups (a)–(c) done; generated guides have no wall height (SplineDef) → 6.0 |
| 4 | 4.3 | Reviewer | Fable 5.2 | 1 | running | — | |
| 4 | 3.4 | Implementer | Opus 5.5 | 1 | done | fmt/clippy clean; 84 spec + own tests pass; coverage pin_input 99.26 %, pin_audio 98.69 %, pin_core 99.46 % | CR-003 pin_core::haptics (additive); SDL3 build headers installed in the container; run-time wiring → 6.0 |
| 4 | 3.4 | Reviewer | Fable 5.2 | 1 | running | — | |
| 4 | 4.6 | Reviewer | Fable 5.2 | 1 | **pass** | gates 1–8 pass; 46 spec + 52 own tests; coverage 96.75 %; byte-identical across processes; images and sound stats checked | N1 temp-dir leak in tests/kit.rs, N2 DECISIONS wording, N3 chevron contrast, N5 --help exit code, N6 SFX attack → small polish round by the Implementer; N4 8×16 glyph legibility, N7 → HUMAN_QUEUE; scenes left to 5.3 |
| 4 | 4.6 | Implementer | Opus 5.5 | polish | done | Orchestrator re-ran clippy (--no-deps) and all 98 pin_assetgen tests: green | N1 temp dirs, N2 wording, N3 chevron contrast, N5 --help, N6 0.5 ms attack. **4.6 complete**; target/sp_4_6 deleted |
| 4 | 5.1 | Implementer | Opus 5.5 | 1 | running | — | |
| 4 | 5.1 | Independent Tester | Fable 5.2 | 1 | running | — | |
| 4 | 2.2-S | Implementer | Opus 5.5 | 1 | done | 309 pin_physics tests; coverage 94.6 %; 5 HEAD digests bit-identical; 0 allocs; 2 rotors+kicker+4 balls 6.2 µs mean (target 50) | MovingBodies trait + step_with_bodies; ExternalForce force_in/ForceContext/ball_added/removed; one mechanical line in spec_ball_determinism.rs |
| 4 | 2.2-S | Reviewer | Fable 5.2 | 1 | running | — | |
| 4 | 4.3 | Reviewer | Fable 5.2 | 1 | **fail** | fmt/clippy clean; pin_tablegen all pass; coverage 95.73 % / 96.18 %; deterministic; D1 verified fixed | B1 lanes 65–82 m emit splines the loader refuses (>65 m); B2 no output-size cap (16 400 posts → glb over GltfLimits); N1–N6 polish; O1 4.1 VFS name-normalisation defect (`.\a:` listed but not exists) found by fuzz → all sent as fix 1 |
| 4 | 4.3 | Implementer | Opus 5.5 | fix 1 | running | — | B1, B2, N1–N6, plus O1 (ownership amendment: pin_assets vfs/zip) |
| 4 | 2.2-S | Reviewer | Fable 5.2 | 1 | **fail** | gates 1–7 pass; 342 tests; coverage 94.62 %; bit identity verified independently (18 digests); perf 6.7 µs mean | B1 contact solve diverges (1e37 m/s) on a finite-mass body at a one-sided stop (take-back deltas use the wrong-side inverse mass); B2 recommended prediction scheme bounces a cradled ball at default flipper numbers (needs an owner-side clamp obligation); N1 kinematic pinch costs ms; N2–N7 docs → fix 1 |
| 4 | 2.2-S | Implementer | Opus 5.5 | fix 1 | running | — | signatures unchanged |
| 4 | 2.7 | Implementer | Opus 5.5 | 1 | running | — | started before 2.2-S passes: force context passed review (N5); shares target/sp_2_2S |
| 4 | 2.7 | Independent Tester | Fable 5.2 | 1 | running | — | |
| 4 | 3.4 | Reviewer | Fable 5.2 | 1 | **fail** | gates 1–7 pass; 84 spec tests; coverage core 99.34 / input 99.26 / audio 98.63 / app 93.57 %; wire format byte-checked; 10× runs clean | B1 RumbleMapper never merges after a tick regression and reset() does not repair it; N1 SSF impulse phase loop unbounded on absurd impulse_hz; N2–N6 docs/edge → fix 1. CR-003 verified additive |
| 4 | 3.4 | Implementer | Opus 5.5 | fix 1 | running | — | |
| 4 | 3.4 | Implementer | Opus 5.5 | fix 1 | done | fmt/clippy clean; all tests incl. 84 spec pass | B1 restart threshold 1000 ticks + reset zeroes latest tick; N1 Nyquist clamp; N2–N6 docs/edge fixes |
| 4 | 3.4 | Reviewer | Fable 5.2 | 2 | running | — | |
| 4 | 5.1 | Independent Tester | Fable 5.2 | 1 | done | 66 spec tests pass (48 GPU-free, 18 GPU on lavapipe); 8 snapshots blessed after analytic checks | no defects; implementer test file had 3 clippy errors at the time → relayed |
| 4 | 2.7 | Independent Tester | Fable 5.2 | 1 | done | 46 spec tests pass | no defects; Magnus hook applies an exact rotation of v (differs from literal S(ω×v) by O(S|ω|h/m)) — tests accept either, Reviewer to confirm |
