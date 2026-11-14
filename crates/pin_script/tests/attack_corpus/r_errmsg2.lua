local s = string.rep("e", 60000) local pc = pcall
function on_tick() while true do pc(pinball.pulse_coil, s) pc(collectgarbage, s) end end
