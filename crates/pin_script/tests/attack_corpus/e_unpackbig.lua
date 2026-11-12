t = {} n = 0
function on_tick()
  if n < 60000 then for i = 1, 6000 do n = n + 1 t[n] = n end
  else while true do pcall(table.unpack, t) pcall(select, '#', table.unpack(t, 1, 20000)) end end
end
