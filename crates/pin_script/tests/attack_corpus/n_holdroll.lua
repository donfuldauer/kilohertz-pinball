n = 0
function on_tick()
  n = n + 1
  if n == 1 then pinball.hold_coil(3, true)
  elseif n == 2 then pinball.hold_coil(3, false) error("after release") end
end
