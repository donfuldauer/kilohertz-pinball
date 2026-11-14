n = 0
function on_tick()
  n = n + 1
  if n == 1 then pinball.hold_coil(3, true) pinball.hold_coil(4, true) pinball.hold_coil(5, true)
  elseif n == 2 then pinball.hold_coil(3, false) pinball.hold_coil(6, true) pinball.hold_coil(4, false) pinball.hold_coil(4, true) while true do end end
end
function on_switch(id, closed) if n == 1 then pinball.hold_coil(5, false) pinball.hold_coil(7, true) error("sw") end end
