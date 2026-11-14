t = {} n = 0 phase = 0
function on_tick()
  if phase == 0 then for i = 1, 100000 do n = n + 1 t[n] = true end if n >= 800000 then phase = 1 n = 0 end
  elseif phase == 1 then for i = 1, 100000 do n = n + 1 t[n] = nil end if n >= 800000 then phase = 2 end
  else local nx = next while true do nx(t) end end
end
