local s = string.rep("x", 65536) local f = "%s"
local pc, sf = pcall, string.format
function on_tick() while true do sf(f, s) end end
