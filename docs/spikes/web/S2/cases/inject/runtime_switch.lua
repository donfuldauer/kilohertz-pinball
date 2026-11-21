-- S2 fault injection: a runtime error in the 50th on_switch call.
do local f = on_switch; local n = 0; on_switch = function(...) n = n + 1 if n == 50 then error('injected switch fault') end if f then return f(...) end end end
