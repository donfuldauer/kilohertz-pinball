local c = collectgarbage function on_tick() while true do pcall(c, "collect") end end
