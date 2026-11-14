keep = {} n = 0
function on_tick() for i = 1, 3000 do n = n + 1 keep[n] = {i, i} end end
