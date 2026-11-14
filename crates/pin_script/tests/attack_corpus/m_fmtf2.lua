local sf = string.format local f = string.rep("%99.99f", 100) function on_tick() while true do pcall(sf, f, 1e300) end end
