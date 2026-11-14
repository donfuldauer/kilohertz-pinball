local s = string.rep("x", 65536) local g = string.gsub function on_tick() while true do pcall(g, s, "x", "%0%0") end end
