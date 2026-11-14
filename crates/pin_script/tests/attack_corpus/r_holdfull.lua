n = 0
function on_tick()
  n = n + 1
  if n <= 3 then for c = (n - 1) * 30, n * 30 - 1 do pinball.hold_coil(c, true) end
  elseif n == 4 then for c = 0, 31 do pinball.hold_coil(c, false) end error("x") end
end
