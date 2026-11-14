-- luaH_getn / hash border search worst cases
t = {} n = 0 phase = 0
function on_tick()
  if phase == 0 then for i = 1, 100000 do n = n + 1 t[n * 2] = true end if n >= 300000 then phase = 1 end
  else local c = 0 while true do c = c + #t end end
end
