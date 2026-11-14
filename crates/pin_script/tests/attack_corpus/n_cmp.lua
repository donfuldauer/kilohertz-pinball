local a = string.rep("a", 65536) local b = string.rep("a", 65535) .. "b"
function on_tick() local c = 0 while true do if a < b then c = c + 1 end if a == b then c = c - 1 end end end
