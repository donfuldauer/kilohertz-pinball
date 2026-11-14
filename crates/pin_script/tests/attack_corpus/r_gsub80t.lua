local s = string.rep("x", 65536) local sub = string.rep("a", 80)
local pc, gs = pcall, string.gsub local r = {a = s}
function on_tick() while true do pc(gs, sub, "a", r) end end
