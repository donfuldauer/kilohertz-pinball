local s = string.rep("e", 60000) local pc, er = pcall, error
function on_tick() while true do pc(er, s, 1) end end
