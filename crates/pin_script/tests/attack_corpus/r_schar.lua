local sc, sb, pc = string.char, string.byte, pcall local s = string.rep("x", 65536)
function on_tick() while true do pc(sc, sb(s, 1, 60000)) end end
