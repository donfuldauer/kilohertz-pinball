local sf = string.format local s = string.rep("%%", 65536) function on_tick() while true do sf(s) end end
