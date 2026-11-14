t = {} n = 0
function on_tick()
  if n < 100000 then for i = 1, 6000 do n = n + 1 t[n] = (n * 7919) % 1000003 end
  else table.sort(t) end
end
