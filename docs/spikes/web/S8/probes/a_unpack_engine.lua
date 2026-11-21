-- (a)/(b) heap-headroom-dependent limits under the engine cap (2 MiB):
-- largest n with table.unpack({}, 1, n) (lua_checkstack fails softly) and
-- largest stack depth a Lua recursion reaches through pcall-free probing.
function on_tick()
  local lo, hi = 0, 1
  local function okn(n) return (pcall(function() return select('#', table.unpack({}, 1, n)) end)) end
  while okn(hi) and hi < (1 << 22) do lo = hi hi = hi * 2 end
  while hi - lo > 1 do local m = (lo + hi) // 2 if okn(m) then lo = m else hi = m end end
  out('unpack_max_n', lo)
  local keep = {}
  for i = 1, 200 do keep[i] = string.rep('q', 1000) .. i end
  lo, hi = 0, 1
  while okn(hi) and hi < (1 << 22) do lo = hi hi = hi * 2 end
  while hi - lo > 1 do local m = (lo + hi) // 2 if okn(m) then lo = m else hi = m end end
  out('unpack_max_n_after_200KB', lo)
end
