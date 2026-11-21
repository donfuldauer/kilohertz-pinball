-- (c)/(g) luaS_new's string cache is indexed by the C pointer (address % 53):
-- luaL_tolstring pushes a metatable's __name with lua_pushstring, so a long
-- __name (> 40 bytes) is either found in the cache (no allocation, no charge)
-- or created again (allocation + KHZ 4 charge). Observable: budget units
-- (EVENTS) and the accounted heap.
local objs = {}
for i = 1, 60 do objs[i] = setmetatable({}, {__name = string.rep('N', 41 + i) .. i}) end
function on_tick()
  local n = 0
  for r = 1, 20 do for i = 1, 60, (r % 7) + 1 do n = n + #tostring(objs[i]) end end
  S8_OUT[1] = tostring(n)
end
