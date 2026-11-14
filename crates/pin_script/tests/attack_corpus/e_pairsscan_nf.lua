t = {} n = 0 phase = 0
function on_tick()
  if phase == 0 then for i = 1, 6000 do n = n + 1 t[n] = true end if n >= 60000 then phase = 1 n = 0 end
  elseif phase == 1 then for i = 1, 6000 do n = n + 1 t[n] = nil end if n >= 60000 then phase = 2 t[60000] = 1 end
  else for r = 1, 10 do for k in pairs(t) do end end end
end
