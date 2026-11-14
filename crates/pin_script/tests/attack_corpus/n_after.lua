function on_tick()
  pcall(function() while true do end end)
  pinball.pulse_coil(9, 9)
end
function on_switch() pinball.pulse_coil(8, 8) end
