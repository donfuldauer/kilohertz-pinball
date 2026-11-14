local s = string.rep("x", 65536) local g = string.gsub function on_tick() while true do g(s, "y", "z") end end
