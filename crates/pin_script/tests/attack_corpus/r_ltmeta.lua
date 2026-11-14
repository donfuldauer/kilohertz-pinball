local a = string.rep("a", 65536) local b = string.rep("a", 65535) .. "b"
local mt = {__lt = function(x, y) return a < b end, __concat = function(x, y) return a end, __eq = function() return a == b end}
local x, y = setmetatable({}, mt), setmetatable({}, mt)
function on_tick() local c = 0 while true do if x < y then c = c + 1 end local z = x .. y end end
