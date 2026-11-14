local s = string.rep("x", 65536) local f = string.rep("%.1s", 250)
local a = {} for i = 1, 250 do a[i] = s end
local pc, sf, un = pcall, string.format, table.unpack
function on_tick() while true do sf(f, un(a)) end end
