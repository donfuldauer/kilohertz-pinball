local s = string.rep("a", 65536) local sb, sc, sel = string.byte, string.char, select
function on_tick() while true do sel('#', sb(s, 1, -1)) end end
