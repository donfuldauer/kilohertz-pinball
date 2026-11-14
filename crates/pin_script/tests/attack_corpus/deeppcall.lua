local function f() pcall(f) end
function on_tick() f() end
