local s = string.rep("x", 65536) local sub = string.sub function on_tick() while true do sub(s, 2) end end
