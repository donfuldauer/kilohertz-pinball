local w = setmetatable({}, {__mode = "k"})
local n = 0 local first = {} local last = first
function on_tick()
  for i = 1, 60000 do local k = {} w[last] = k last = k n = n + 1 if n > 1e9 then break end end
end
