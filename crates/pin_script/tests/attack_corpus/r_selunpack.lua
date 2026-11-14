local t = {} for i = 1, 5000 do t[i] = i end local sel, un = select, table.unpack
function on_tick() while true do sel('#', un(t)) end end
