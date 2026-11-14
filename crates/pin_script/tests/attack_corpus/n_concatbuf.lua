local s = string.rep("x", 65536) local t = {} for i = 1, 200 do t[i] = s end
local pc, tc = pcall, table.concat
function on_tick() while true do pc(tc, t) end end
