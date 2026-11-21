-- S2 fault injection: a memory bomb in on_tick at tick 20000 (2 MiB cap).
do local f = on_tick; on_tick = function(...) if pinball.tick() == 20000 then local t = {} for i = 1, 100 do t[i] = string.rep('m', 65000) end end return f(...) end end
