local t = {} for i = 1, 4000 do t[i] = (i * 7919) % 4001 end
local ts, ult, pc = table.sort, math.ult, pcall
function on_tick() while true do pc(ts, t, ult) end end
