local s = string.rep("a", 5000000) local s2 = string.rep("a", 4999999) .. "a"
function on_tick() local c = 0 for i = 1, 300000 do if s == s2 then c = c + 1 end end end
