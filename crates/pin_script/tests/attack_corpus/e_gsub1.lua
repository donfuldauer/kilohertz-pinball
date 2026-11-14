local s = string.rep("x", 65536) local pc, gs = pcall, string.gsub
function on_tick() while true do gs(s, "x", "y") end end
