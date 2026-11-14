local sf = string.format local s = string.rep("\0", 65536) function on_tick() while true do pcall(sf, "%q", s) end end
