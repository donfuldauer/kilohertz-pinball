keep = {} n = 0 local P = string.rep("x", 1000) local rep = string.rep
function on_tick() while true do pcall(function() for i = 1, 100 do n = n + 1 keep[n] = rep(P, 60) .. n end end) end end
