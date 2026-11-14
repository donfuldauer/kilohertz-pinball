function on_tick() local t = {} local i = 0 while true do i = i + 1 local a, b, c, d, e, f, g, h = i, i, i, i, i, i, i, i t[i] = function() return a + b + c + d + e + f + g + h end end end
