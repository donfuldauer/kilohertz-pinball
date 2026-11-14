local t = {} for i = 1, 50000 do t[i] = (i * 7919) % 50021 end
local ts, ult = table.sort, math.ult
function on_tick() while true do pcall(ts, t, ult) end end
