local s = string.rep("x", 65536) local f = string.rep("%s", 200) local t = {} for i = 1, 200 do t[i] = s end
local pc, sf, un = pcall, string.format, table.unpack
function on_tick() while true do pc(sf, f, un(t)) end end
