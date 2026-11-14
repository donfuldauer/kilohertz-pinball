# Lua sandbox attack corpus

Scripts the 4.2 Reviewer used to try to escape or stall the Lua sandbox, kept as data for future reviews (review finding N13). They are not run by `cargo test`; the `spec_stall.rs`, `spec_hardening.rs` and `lua_backend.rs` tests cover each class.

`RESULTS_engine_config.txt` is the Reviewer's per-script worst-tick measurement under `LuaConfig::engine()` (100,000 instructions, 2 MiB), release build, commit dfd52ec. Target: every tick ≤ 8 ms (DECISIONS "Orchestrator — 4.2 re-review round 3"). Measured worst: 5.7 ms.

Prefixes: no prefix = round 1, `n_`/`m_` = round 2, `r_` = round 3, `e_` = sized for the engine config.

`savebomb.lua` was later resized from `string.rep("x", 8000000)` to `string.rep("x", 65536)` (exactly the 64 KiB string cap): the original never loaded, so its save hook was never exercised. It now loads, and on reload its saved state is dropped by the save byte budget (`lua_backend.rs::corpus_savebomb_loads_and_its_state_is_dropped_on_reload`). Its `RESULTS_engine_config.txt` line and DECISIONS table row record the original file.
