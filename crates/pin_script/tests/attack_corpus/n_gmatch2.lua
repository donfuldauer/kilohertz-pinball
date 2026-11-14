local s = string.rep("a", 65536) local gm = string.gmatch
function on_tick() while true do local it = gm(s, "a*a*a*a*b") pcall(it) end end
