t = {} n = 0 phase = 0
function on_tick()
  if phase == 0 then for i = 1, 4000 do n = n + 1 t[n * 2] = true end if n >= 30000 then phase = 1 end
  else local c = 0 local t = t while true do c = c + #t end end
end
