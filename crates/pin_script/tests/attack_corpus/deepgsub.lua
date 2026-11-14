local function f(s) return (string.gsub("x", "x", f)) end
function on_tick() f("x") end
