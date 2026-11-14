local mt = {__name = string.rep("n", 60000)} local x = setmetatable({}, mt) local ts = tostring
function on_tick() while true do ts(x) end end
