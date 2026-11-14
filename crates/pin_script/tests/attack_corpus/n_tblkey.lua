local a = string.rep("a", 65536) local b = string.rep("a", 65536) local t = {[a] = 1}
function on_tick() local c = 0 while true do c = c + (t[b] or 0) end end
