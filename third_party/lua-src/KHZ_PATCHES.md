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
| 3 | `lua.h`, `lstate.h`, `lstate.c` | Cost budget per state (`global_State.khzbudget`) with `lua_khzsetbudget`, `lua_khzspend` (non-raising, used by the host hook) and `lua_khzcharge` (raises "instruction budget exceeded" when it goes negative and sets the count hook to fire on every instruction). Unlimited until the host sets it. |
| 4 | `lstring.c` | Creating a string longer than `KHZ_MAX_STRLEN` (64 KiB) raises "string length exceeds the 64 KiB limit"; each long-string creation charges `len/64 + 1`. |
| 5 | `lstrlib.c` | Pattern matching counts steps per call (`KHZ_MATCH_STEPS` = 1 000 000) and raises "pattern too complex" past it; every 1024 steps are charged. Plain `find` charges its scan and each candidate compare. `string.rep` returns `""` for empty pieces (no empty loop) and refuses results over 64 KiB before looping. `string.byte` and `string.unpack` charge by size. `string.format('%p')` raises. |
| 6 | `ltablib.c` | `table.insert`/`remove` (shifts), `move`, `concat`, `unpack` charge 4 per element before looping; `sort` charges each partition pass. |
| 7 | `lutf8lib.c` | `utf8.len`, `codepoint`, `offset` charge by the scanned length. |
| 8 | `lvm.c` | Long-string `==`, `<`, `<=` charge `len/64`; string→number coercion refuses strings over 200 bytes; `OP_TBC` and a non-false 4th generic-`for` value raise "to-be-closed ... not allowed". |
| 9 | `lparser.c` | The `<close>` attribute is a compile error: Lua runs `__close` handlers with hooks off after an error raised from a hook. |
| 10 | `lauxlib.c` | `luaL_tolstring` (`tostring`, `%s`) prints the type name only for tables, functions, userdata and threads (no addresses). |
| 11 | `lbaselib.c` | `tonumber` refuses strings over 200 bytes (bounded scan). |

The host side is `crates/pin_script/src/lua/` (Subphase 4.2).
