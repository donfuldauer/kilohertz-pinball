local pc, rep = pcall, string.rep local s = string.rep("x", 60000)
function on_tick() while true do pc(rep, s, 2, s) end end
