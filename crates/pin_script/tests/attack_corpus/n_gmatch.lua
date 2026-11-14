local s = string.rep("a", 65536)
function on_tick() while true do for m in s:gmatch(".-.-.-b") do end end end
