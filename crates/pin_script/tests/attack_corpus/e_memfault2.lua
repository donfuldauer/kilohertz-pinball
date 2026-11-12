keep = {} n = 0 local P = string.rep("x", 1000) local rep = string.rep
function on_tick() for i = 1, 20 do n = n + 1 keep[n] = rep(P, 60) .. n end end
