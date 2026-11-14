local t = {} for i = 1, 200 do t[i] = 0x7FFFFFFF end
local pc, uc, un = pcall, utf8.char, table.unpack
function on_tick() local s = uc(un(t)) while true do local x = uc(un(t)) end end
