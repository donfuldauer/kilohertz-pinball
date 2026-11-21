-- S2 fault injection: a runtime error in on_tick at tick 20000.
do local f = on_tick; on_tick = function(...) if pinball.tick() == 20000 then error('injected fault') end return f(...) end end
