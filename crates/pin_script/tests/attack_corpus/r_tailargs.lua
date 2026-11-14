local function g(...) return select('#', ...) end
local function f(...) while true do g(...) end end
local t = {} for i = 1, 8000 do t[i] = i end
function on_tick() f(table.unpack(t)) end
