local s = string.rep("x", 65536) local sub = string.rep("a", 200)
local pc, gs = pcall, string.gsub
function on_tick() while true do pc(gs, sub, "a", s) end end
