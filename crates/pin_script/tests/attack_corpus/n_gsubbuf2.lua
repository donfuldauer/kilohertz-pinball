local s = string.rep("x", 65000) local sub = string.rep("a", 240)
local pc, gs = pcall, string.gsub
function on_tick() while true do pc(gs, sub, "a", {a = s}) end end
