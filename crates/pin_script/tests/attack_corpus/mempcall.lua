function on_tick() local t = {} while true do pcall(function() for i = 1, 1e9 do t[#t + 1] = string.rep("x", 1e6) .. i end end) end end
