local mt = {} mt.__concat = function(a, b) return a .. b end
function on_tick() local a = setmetatable({}, mt) return a .. a end
