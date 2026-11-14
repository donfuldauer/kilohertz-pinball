# Kilohertz Pinball patches to lua-src 551.0.2 (Lua 5.4.9)

Vendored from crates.io `lua-src =551.0.2` and wired in with `[patch.crates-io]`
in the workspace `Cargo.toml`. Only `lua-5.4.9/` is kept (the other Lua versions
were removed; the crate's build logic is unchanged). Every edit is plain C89/C99
and marked `KHZ` in the source. Rationale: docs/DECISIONS.md, "Orchestrator — 4.2
review: patched vendored Lua" and "4.2 — fix round 1".

| # | File | Patch |
|---|------|-------|
| 1 | `lstate.c` | `luai_makeseed` is a constant: string hashes and `pairs` order are the same in every process. |
| 2 | `ltablib.c` | `l_randomizePivot` is `0`: `table.sort` is reproducible (no `clock()`/`time()`). |
| 3 | `lua.h`, `lstate.h`, `lstate.c` | Cost budget per state (`global_State.khzbudget`): `lua_khzsetbudget`, `lua_khzspend` (non-raising; the host's count hook spends its interval with it, and it drains the thread-local pending cost of patch 4) and `lua_khzcharge` (raises "instruction budget exceeded" when the budget goes from ≥ 0 to < 0, and sets the count hook to fire on every instruction). Unlimited until the host sets it. Cost constants `KHZ_CCALL_COST` (32), `KHZ_ERROR_COST` (1024), `KHZ_FLOAT_COST` (256), `KHZ_MAX_STRLEN` (64 KiB), `KHZ_MATCH_STEPS` (1 000 000). |
| 4 | `lstring.h`, `lstring.c` | Creating a string longer than 64 KiB raises "string length exceeds the 64 KiB limit"; each long-string creation charges `len/64 + 1`. Long-string compares (`luaS_eqlngstr`, also used by table lookups, which have no `lua_State`) add `len/64` to a thread-local pending cost (`KHZ_TLS`: C11 `_Thread_local`, MSVC `__declspec(thread)`). |
| 5 | `lstrlib.c` | Pattern matching: at most 1 000 000 steps per call (reset per `gmatch` step), then "pattern too complex"; 2 units per step. Plain `find` charges its scan and each candidate compare. `string.rep` refuses results over 64 KiB before looping (an ordinary error); for empty pieces it returns `""` without looping but charges `n/16`. `byte`, `unpack`, `lower`/`upper`/`reverse` charge by size. String-arithmetic coercion refuses strings over 200 bytes and charges their length. `format` charges `len(fmt)/4`, 256 per `%a %e %f %g`, `len/16` per `%s`, `2·len` per `%q` string (256 per `%q` number); `%p` raises. |
| 6 | `ltablib.c` | `table.insert`/`remove` (shifts), `move`, `concat`, `unpack` charge 4 per element (saturating) before looping; `sort` charges each partition pass. |
| 7 | `lutf8lib.c` | `utf8.len`, `codepoint`, `offset` charge by the scanned length. |
| 8 | `lvm.c` | Long-string `<`, `<=` charge `len/64`; string→number coercion refuses strings over 200 bytes and adds `len + 16` to the pending cost; float `%` (`luaV_modf`) charges 256; `OP_TBC` and a non-false 4th generic-`for` value raise "to-be-closed ... not allowed". |
| 9 | `lparser.c` | The `<close>` attribute is a compile error: Lua runs `__close` handlers with hooks off after an error raised from a hook. |
| 10 | `lauxlib.c` | `luaL_tolstring` (`tostring`, `%s`) prints only the type name for tables, functions, userdata and threads (no addresses). |
| 11 | `lbaselib.c` | `tonumber` refuses strings over 200 bytes and charges `len + 16` (`4·len + 16` with a base). |
| 12 | `ldebug.c` | Raising an error charges 1024 (`luaG_errormsg`). |
| 13 | `ltm.c` | Expanding `...` charges `n/4` (`luaT_getvarargs`). |
| 14 | `ldo.c` | Every C-function call (library or host function) charges 32 (`precallC`). |
| 15 | `lobject.c` | Number→string conversion charges 256 for floats, 16 for integers (`luaO_tostring`, `%f` in `luaO_pushvfstring`). |
| 16 | `lmathlib.c` | Float `math.fmod` charges 256. |
| 17 | `lstate.c`, `lstate.h` | `luaE_khzdefer`: charge without raising (arms the count hook if the budget went negative), for code that must not throw anything but memory errors. |
| 18 | `ltable.c` | `luaH_next` charges skipped slots / 8; `luaH_resize` charges (old + new array and hash sizes) / 16 — both deferred. |
| 19 | `lgc.c` | A full collection (`luaC_fullgc`, including emergency collections inside an allocation) charges total heap bytes / 64, deferred. |
| 20 | `lauxlib.c` | A `luaL_Buffer` may not grow past 64 KiB + 1 KiB slack ("string length exceeds the 64 KiB limit", raised as soon as it would) and each growth charges new size / 64 — covers `table.concat`, `gsub`, `format`, `rep`, `string.char`, `utf8.char` and every other buffer user. |
| 21 | `lstrlib.c` | `string.rep` builds its result by doubling `memcpy` (O(log n) calls) and charges total length / 64 + 1. |
| 22 | `src/lib.rs` | `KHZ_PATCHED_SOURCES` `include_str!`s every patched file so cargo rebuilds Lua when they change (cargo tracks a path dependency through rustc's dep-info only). |
| 23 | `ldo.c` | `tryfuncTM` charges (stack slots shifted) / 8 before opening the slot for a `__call` metamethod. A cyclic `__call` chain retries it from `luaD_precall`/`luaD_pretailcall` without running an instruction, shifting one more slot each time. |
| 24 | `lstrlib.c` | Pattern scans below one matching step count steps (`khz_steps`, same 1 000 000 limit, charged 2 per step like `khz_step`): `%b` scans and bracket-class scans (`classend`, `singlematch`) 1 step per 16 bytes, `%f` 1 per 8 (two class scans), back-reference (`%1`) compares 1 per 64 bytes. |
| 25 | `ltable.c` | Hash-chain walks in `luaH_getint`, `luaH_getshortstr`, `getgeneric` and the "find previous" walk in `luaH_newkey` add probes / 8 to the thread-local pending cost (patch 4) when a walk takes ≥ 16 probes. Integer hashes are a plain modulo and string hashes use a fixed seed (patch 1), so a script can build one long chain. Lookups have no `lua_State` and must not throw: the cost reaches the budget at the next count-hook call, so one hook interval of such lookups can still run before the fault. Ordinary tables never reach 16 probes, so this costs one counter on the hot path. |
| 26 | `lstring.c`, `lstring.h`, `lstate.c` | The pending cost of patch 4 belongs to one state (`luaS_khzowner`); another state's spend never takes it. Amended by patch 28: it used to be discarded when another state spent, and the owner was the last state to set or spend a budget rather than the one that produced the cost. |
| 27 | `lgc.c` | `__mode` is ignored: `traversetable` treats every table as strong (the weak/ephemeron branch is kept but dead). The sandbox's `setmetatable` refuses `__mode`, but a script can add it to the metatable afterwards, and weak and ephemeron tables are cleared and re-traversed in the GC's atomic step, outside the budget. Only the sandbox prelude's `protected` proxy set was weak-keyed; its proxies live as long as the state, so it is unaffected. mlua's only weak table (per-thread hooks, `Thread::set_hook`) is not used by `pin_script`. (CodeRabbit review of PR #5; test `crates/pin_script/tests/cr_weak_tables.rs`.) |
| 28 | `lstring.c`, `lstring.h`, `lstate.c`, `lstate.h`, `lapi.c`, `ldo.c`, `lzio.c`, `src/lib.rs` | Pending cost is kept per state. `global_State.khzpending` parks a state's pending cost while another state owns the thread-local accumulator; `luaS_khzclaim` (behind the inline check `luaS_khzown`) moves the accumulator to the new owner and parks the old owner's cost in its `khzpending`, which its next `lua_khzspend` charges. Ownership is claimed on every entry into a state (`lua_callk`, `lua_pcallk`, `lua_load`, `lua_resume`, `lua_khzsetbudget`, `lua_khzspend`) and on every return to it from code that may have run another state (after the C function in `precallC`, after both C continuation calls in `finishCcall` and `unroll`'s yield path, after the chunk reader in `luaZ_fill`, after the hook in `luaD_hook`, and in `luaD_throw`), so cost accrued while a state runs is charged to that state only; `lua_khzsetbudget` clears only its own state's pending cost. `lua_close` releases ownership, so the owner pointer never dangles provided a state is closed on the thread it last ran on (mlua's `Lua` is `!Send` without its `send` feature, which is off). The steady-state cost is one thread-local compare per C-function call. (CodeRabbit review of PR #3; test `crates/pin_script/tests/cr_lua_budget.rs`.) |
| — | `Cargo.toml` | Test target and dev-dependencies removed; only `lua-5.4.9/` kept. |

Budget units are roughly "one VM instruction" (a few ns). All edits compile warning-free with `gcc -std=gnu99 -Wall -Wextra`.

The host side is `crates/pin_script/src/lua/` (Subphase 4.2). Patches 23–26 and 28 come from the CodeRabbit review of PR #3; their regression tests are `crates/pin_script/tests/cr_lua_budget.rs`.
