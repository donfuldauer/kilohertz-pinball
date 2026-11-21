-- S2 fault injection: an infinite loop in on_tick at tick 20000 (budget kill).
do local f = on_tick; on_tick = function(...) if pinball.tick() == 20000 then while true do end end return f(...) end end
