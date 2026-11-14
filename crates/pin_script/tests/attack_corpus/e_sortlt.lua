local mt = {} mt.__lt = function(a, b) local g = {a, b, {}, {}} return a.v < b.v end
local t, n = {}, 0
function on_tick()
  if n < 3000 then for i = n + 1, n + 500 do t[i] = setmetatable({v = (i * 7919) % 3001}, mt) end n = n + 500
  else table.sort(t) end
end
