local function f(...) return {...} end local t = {} for i = 1, 200 do t[i] = i end local un = table.unpack function on_tick() while true do f(un(t)) end end
