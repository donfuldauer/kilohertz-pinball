local function f(...) while true do local t = {...} end end
local t = {} for i = 1, 5000 do t[i] = i end
function on_tick() f(table.unpack(t)) end
