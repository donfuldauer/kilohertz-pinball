//! Regression test for CodeRabbit PR #5 (sandbox.rs `__mode` guard): the
//! sandbox's `setmetatable` refuses `__mode`, but a script can add it to the
//! metatable afterwards (or reach a table through a metatable that has its own
//! metatable). Weak and ephemeron tables are processed in the GC's atomic step,
//! outside the budget, so the patched Lua ignores `__mode` altogether
//! (third_party/lua-src/KHZ_PATCHES.md, patch 27): every table is strong.

mod spec_support;

use spec_support::{Host, backend_with, cfg};

const MIB: usize = 1024 * 1024;

const SCRIPT: &str = r#"
local mk, mv = {}, {}
local wk = setmetatable({}, mk)
local wv = setmetatable({}, mv)
mk.__mode = "k"   -- added after setmetatable: the sandbox guard cannot see it
mv.__mode = "v"
wk[{}] = true                  -- weak key, referenced nowhere else
do local k = {} wk[k] = { k } end  -- ephemeron: the value refers to its key
wv[1] = {}                     -- weak value, referenced nowhere else
-- A table whose metatable's own metatable carries __mode.
local mm = setmetatable({}, setmetatable({}, { __index = { } }))
getmetatable(mm).__mode = "kv"
mm[{}] = {}

local function entries(t) local n = 0 for _ in pairs(t) do n = n + 1 end return n end

function on_tick()
  for i = 1, 2000 do local garbage = { i } end  -- drive the incremental GC
  if entries(wk) ~= 2 then error("weak-keyed entry collected") end
  if wv[1] == nil then error("weak-valued entry collected") end
  if entries(mm) ~= 1 then error("all-weak entry collected") end
  pinball.show_scene(1)
end
"#;

#[test]
fn weak_and_ephemeron_modes_have_no_effect() {
    // Each tick allocates over 100 KiB of garbage, so 300 ticks allocate far
    // more than the 4 MiB heap cap: the collector must complete many cycles
    // (or the script would fault on memory), and every one of them would have
    // cleared the weak entries if `__mode` were honoured.
    let mut b = backend_with(SCRIPT.as_bytes(), cfg(4 * MIB, 1_000_000));
    let mut h = Host::new(1);
    for _ in 0..300 {
        h.tick(&mut b);
        assert!(!b.is_faulted(), "a weak entry was collected (or memory ran out)");
        assert_eq!(h.cmd.display_scenes.len(), 1);
    }
}
