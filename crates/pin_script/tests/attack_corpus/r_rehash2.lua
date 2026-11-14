t = {} n = 0 phase = 0 k = 0
local N = 16384
function on_tick()
  if phase == 0 then for i = 1, 1500 do if n < N then n = n + 1 t["k" .. n] = true end end if n >= N then phase = 1 k = n end
  else local t = t while true do t[-k] = true t[-k] = nil k = k + 1 end end
end
