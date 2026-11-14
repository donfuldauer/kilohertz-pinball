# Lua sandbox attack corpus

Scripts the 4.2 Reviewer used to try to escape or stall the Lua sandbox, kept as data for future reviews (review finding N13). They are not run by `cargo test`; the `spec_stall.rs`, `spec_hardening.rs` and `lua_backend.rs` tests cover each class.

`RESULTS_engine_config.txt` is the Reviewer's per-script worst-tick measurement under `LuaConfig::engine()` (100,000 instructions, 2 MiB), release build, commit dfd52ec. Target: every tick ≤ 8 ms (DECISIONS "Orchestrator — 4.2 re-review round 3"). Measured worst: 5.7 ms.

Prefixes: no prefix = round 1, `n_`/`m_` = round 2, `r_` = round 3, `e_` = sized for the engine config.

`savebomb.lua` was later resized from `string.rep("x", 8000000)` to `string.rep("x", 65536)` (exactly the 64 KiB string cap): the original never loaded, so its save hook was never exercised. It now loads, and on reload its saved state is dropped by the save byte budget (`lua_backend.rs::corpus_savebomb_loads_and_its_state_is_dropped_on_reload`). Its `RESULTS_engine_config.txt` line and DECISIONS table row were re-measured after the resize (release build, `LuaConfig::engine()`, commit 3d9ffc3): worst tick 8.6 µs over 600 ticks, no fault, and worst of 20 reloads (which run the save hook) 2.03 ms. The original file failed to load (0.225 ms).

Six more files were resized later because under `LuaConfig::engine()` they stopped before the code they target ran; `lua_backend.rs` now checks each one reaches it (`corpus_e_sortbig_reaches_the_sort`, `corpus_e_sortlt_reaches_the_comparator`, `corpus_m_fmtlit2_loads_and_formats`, `corpus_r_rehash_reach_phase_1`: the file as given faults at the same tick as a copy with a probe in the targeted code). Their `RESULTS_engine_config.txt` lines and DECISIONS rows (`e_*` have none) were re-measured after the change (release build, `LuaConfig::engine()`, 600 ticks, code at commit ea26206); no tick reached 8 ms:

- `e_sortbig.lua`: 100 000 → 60 000 entries (the array part's growth to 131 072 slots hit the 2 MiB heap at tick 10, before `table.sort`). Now the sort runs at tick 10 and faults with `BudgetExceeded` after 27 µs; worst tick 0.68 ms (set-up).
- `e_sortlt.lua`: the 3 000 objects are built 500 per tick instead of at top level (where they ran out of budget, so `new` failed and `__lt` never ran). Now `table.sort` runs at tick 6 and faults with `BudgetExceeded` inside the allocating comparator after 2.02 ms (the worst tick).
- `m_fmtlit2.lua`: `string.rep("%%", 65536)` (128 KiB, over the string cap, failed to load) → `string.rep("%%", 32768)` (exactly 64 KiB). Its `string.format` loop runs and faults with `BudgetExceeded` at tick 0 after 0.14 ms.
- `r_rehash.lua` N 131 072 → 8 192, `r_rehash2.lua` and `r_rehash3.lua` N 262 144 → 16 384: the originals faulted in set-up (`MemoryLimit` at tick 4, `BudgetExceeded` at tick 16, `MemoryLimit` at tick 4), so phase 1 never ran; 16 384 is the largest power of two whose phase 1 fits the 2 MiB heap (at 32 768 the first phase-1 rehash needs both node arrays and fails with `MemoryLimit`). Now each faults in phase 1 with `BudgetExceeded`: worst ticks 1.04 ms (tick 2), 1.52 ms (tick 11), 1.28 ms (tick 3).
