function on_tick() local t = {} local i = 0 while true do i = i + 1 t[i] = function() return i end end end
