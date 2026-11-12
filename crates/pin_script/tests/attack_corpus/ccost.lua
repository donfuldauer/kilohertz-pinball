local s = string.rep("a", 4000000)
function on_tick() for i = 1, 300000 do s:find("b", 1, true) end end
