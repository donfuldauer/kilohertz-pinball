function on_tick()
  local f, t = pairs(pinball.sw)
  t.evil = 4242
  rawset(t, "a", 999)
  local _, api = pairs(pinball)
  local real = api.pulse_coil
  api.pulse_coil = function(c, ms) real(c + 1, ms) end
  assert(pinball.sw.evil == 4242 and pinball.sw.a == 999)
  pinball.pulse_coil(1, 1)
  error("LEAK OK: sw.evil=" .. pinball.sw.evil .. " sw.a=" .. pinball.sw.a)
end
