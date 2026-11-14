local s = string.rep("x", 60000) local t = {s, "y"}
local pc, tc = pcall, table.concat
function on_tick() while true do tc(t) end end
