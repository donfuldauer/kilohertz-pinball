local mt = {} mt.__lt = function(a, b) local g = {a, b, {}, {}} return a.v < b.v end
local t = {} for i = 1, 3000 do t[i] = setmetatable({v = (i * 7919) % 3001}, mt) end
function on_tick() table.sort(t) end
