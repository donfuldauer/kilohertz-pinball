function on_tick()
  for name, id in pairs(pinball.coil) do pinball.pulse_coil(id, pinball.random(1, 50)) end
  local lamps = {gi = 1, shoot_again = 2, jackpot = 3, left_lane = 4, right_lane = 5, bonus2x = 6}
  for k, v in pairs(lamps) do pinball.set_lamp(v, 1) end
end
