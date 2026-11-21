# S8 (web-spike-lua-audit): local decisions (PLAN §2.3 rule 8; not in the repo)

## S8-1 — Baseline and variants
**Decision:** v0 = S2's recommended build (KHZ 29-32 + `KHZ_LUA_MATH` + pin_script `web-parity`), rebuilt
from a copy of `S2/repo`; v1 = v0 + `KHZ_S8_FIXES` (KHZ 35-37) + `KHZ_LOGICAL_MEM` (KHZ 33); v2 = v1 +
`KHZ_KEY_ORDER` (KHZ 34). Each patch sits behind a lua-src build knob so one tree builds all three.
Production replays are compared with S1's native recordings (made with the unpatched engine).
**Rationale:** "before" must be S2's result (v0 reproduces S2's sandbox file byte for byte, `mem=` fields
included); knobs let every claim be attributed to one patch; S1's recordings are the untouched engine.
**Alternatives:** separate trees per variant (more disk, harder to diff); re-recording natively (would
hide native changes).

## S8-2 — Memory accounting: option (ii), logical sizes passed at the allocation sites
**Decision:** KHZ 33 passes an explicit logical (LP64) size next to the real size at every luaM_/luaC_newobj
site whose element/object size differs between ILP32 and LP64 (Table, UpVal, Proto, CallInfo, LX, LG,
LocVar, Upvaldesc, Labeldesc, TString*/Proto* vectors, string/userdata/closure headers); GC debt,
`GCestimate`, LUA_GCCOUNT and KHZ 19 use the logical bytes; the cap is checked on logical bytes inside
lmem.c (an over-cap allocation fails like an allocator failure, so Lua's emergency collection and retry run
unchanged); lauxlib buffer boxes allocate through the core; the host allocator keeps a real-byte backstop at
2 x the cap. Types whose size is identical on every target (TValue, StackValue, Node, Instruction,
AbsLineInfo, Vardesc, char) keep real sizes, enforced by compile-time checks; on LP64 the logical constants
are compile-time-checked against the real sizes.
**Rationale:** (i) sizes alone cannot recover types (on wasm32 a 32-byte block is a Table or a 15-byte
string; userdata payloads are Rust type sizes); (iii) size-identical layouts cannot cover pointer vectors
(TString*, Proto*, LClosure upvals) or Rust userdata payloads without patching mlua, and would raise wasm
memory by ~40 %. (ii) is exact by construction and testable (per-callback accounted heap compared native vs
wasm: identical over 804 callbacks).
**Alternatives:** wasm64/memory64 for a separate Lua instance (LP64 layout, but mlua's Rust userdata are
still compiled for wasm64 and the browser support is S5's); accepting the residual (S2-6).

## S8-3 — Full-userdata payloads charged a constant (64 bytes)
**Decision:** every full userdata is charged header (LP64) + 64 bytes whatever its payload.
**Rationale:** payloads are Rust type sizes chosen by mlua (8/32/24/16/72 bytes on x86_64, 4/32/12/8/36 on
wasm32: no fixed ratio), so no size-based rule is platform independent; a constant is, and survives mlua
upgrades as long as the sequence of userdata creations is the same.
**Alternatives:** 72 (the largest x86_64 payload, so logical >= real on LP64; equally valid, changes more
native numbers); a per-type table keyed by the wasm32 size (breaks on any mlua change).

## S8-4 — NaN text: "nan" for every NaN (KHZ 36)
**Decision:** tostring, `..` and string.format %a %A %e %E %f %g %G format a NaN with its sign cleared.
**Rationale:** the sign of an arithmetic NaN is CPU dependent (x86: 0/0 negative; ARM, ARM64 V8 and aarch64:
positive; measured). Clearing the sign makes x86, ARM and wasm identical with the least code; "nan" is what
ARM and macOS arm64 already print.
**Alternatives:** "-nan" for every NaN (keeps x86's text for 0/0, changes ARM's and negated NaNs); a
canonical NaN in arithmetic (touches the VM's hot path).

## S8-5 — Address-free keys: creation ids; light C functions and light userdata share one main position (KHZ 34)
**Decision:** tables, closures, full userdata and threads carry a per-state creation id (32-bit, wraps
deterministically) used as their hash; light C functions and light userdata hash to one main position.
**Rationale:** ids are deterministic and never depend on the allocator; the id fits LP64 padding for
closures and userdata (Table and lua_State grow by 8 bytes on x86_64; logical sizes unchanged); light C
functions are code addresses with no object to carry an id, and there are ~110 of them in the sandbox, so
one chain is bounded (KHZ 25 charges long walks).
**Alternatives:** a side map pointer->id (allocation, GC interaction); keep the documented rule only.

## S8-6 — String cache indexed by content (KHZ 37)
**Decision:** `luaS_new`'s cache bucket is `luaS_hash(str) % STRCACHE_N` instead of the C pointer.
**Rationale:** the address decided hits for long strings (e.g. a long `__name` pushed by `tostring`), which
changed allocations and KHZ 4 charges between platforms (measured even with KHZ 33/34); hashing keeps the
cache and makes it deterministic.
**Alternatives:** bypass the cache (always `luaS_newlstr`): equally deterministic, slower for the C API's
field names.

## S8-7 — Per-tick aggregate budget in the backends, 2 x the per-callback budget, fault on exhaustion
**Decision:** recommend 200 000 units per tick (Lua: VM instructions incl. the final partial hook interval +
KHZ charges; Wasm: wasmtime fuel), shared by a tick's `on_switch_edge` calls and `on_tick` (not `on_init`, not
the host GC step); each callback runs with min(100 000, left); a callback that finds 0 left, or runs out, is a
`BudgetExceeded` fault (tilt). Prototyped inside the backends keyed by `HostView::tick`, so no pin_core
contract change is needed.
**Rationale:** deterministic (units are), same fault model as today's budget kill (rollback, coil release),
no lost or deferred edges to reason about; 200 000 is 314x (Lua) / 125x (Wasm) above the largest production
tick and lets a full-budget on_tick follow a full budget of edges.
**Alternatives:** skip the remaining callbacks (rules silently miss switches); defer edges to the next tick
(changes timing, needs storage, can overflow MAX_PENDING_EDGES); 1x (100 000: a full-budget on_tick could
fault after a few edges).

## S8-8 — Audit depth without Fable
**Decision:** the audit was done at Opus depth (D42: Fable allowance unknown means no Fable): every
allocation site and every sizeof/size_t/long/pointer cast of the compiled sources was enumerated by grep and
the hits read line by line; the rest was covered by black-box probes on four engines.
**Rationale:** brief. **Alternatives:** none (no Fable).

## S8-9 — Probe budgets
**Decision:** audit probes run with a 16 MiB heap and a 400 000 000-unit budget ("big") except the ones
that target the engine cap (2 MiB) or the budget itself ("engine", "engine-bigbudget").
**Rationale:** probes print hundreds of lines; the engine limits would truncate them; the limits are probed
separately. **Alternatives:** split every probe into budget-sized pieces.

## S8-10 — Wasm-rules web check in Node only
**Decision:** the Wasm-rules part of deliverable 4 (per-tick fuel, cap, adversarial tick) runs natively and
in Node with the S8 copy of S4's harness; Chromium is covered by S4's per-callback fuel parity (identical on
every engine) and by the Lua-side Chromium runs.
**Rationale:** time box; the cap rule is the same Rust code on every target and fuel is engine independent.
**Alternatives:** stage S4's page with the S8 copy (≈ 0.25 agent-run).
