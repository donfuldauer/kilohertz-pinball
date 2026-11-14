local t = {} local mt = {__index = function(t, k) return t[k] end}
setmetatable(t, mt)
function on_tick() return t.x end
