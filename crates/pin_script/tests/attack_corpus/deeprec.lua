local function f(n) return f(n + 1) + 1 end
function on_tick() f(1) end
