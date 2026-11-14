local s = string.rep("a", 5000000)
function on_tick() for i = 1, 300000 do local x = s .. "b" end end
