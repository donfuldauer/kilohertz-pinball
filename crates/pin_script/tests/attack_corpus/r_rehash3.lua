t = {} n = 0 phase = 0 k = 0
local N = 262144
function on_tick()
  if phase == 0 then for i = 1, 8000 do if n < N then n = n + 1 t[n + 0.5] = true end end if n >= N then phase = 1 k = n end
  else local t = t while true do t[k - N + 0.5] = nil k = k + 1 t[k + 0.5] = true end end
end
