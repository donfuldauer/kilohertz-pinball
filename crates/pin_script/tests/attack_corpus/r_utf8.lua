local uc, un, pc = utf8.char, table.unpack, pcall local t = {} for i = 1, 15000 do t[i] = 0x7FFFFFFF end
function on_tick() while true do pc(uc, un(t)) end end
