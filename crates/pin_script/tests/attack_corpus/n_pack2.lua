local f = string.rep("i16", 4000) local t = {} for i = 1, 4000 do t[i] = 1 end
local sp, un, ps = string.pack, table.unpack, string.packsize
function on_tick() while true do ps(f) end end
