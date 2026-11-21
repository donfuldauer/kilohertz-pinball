-- (j) the Rust side: argument conversion and error texts of the pinball API
-- (mlua i64/f64 conversions), seen by the script through pcall.
function on_tick()
  local mi, ma = math.mininteger, math.maxinteger
  local args = {0, 1, -1, 65535, 65536, 4294967296, ma, mi, 1.0, 1.5, 2^63, -2^63, 2^64, 0/0, 1/0, '1', 'x', true, {}, nil}
  for i = 1, 20 do
    local a = args[i]
    out('pulse', i, try(pinball.pulse_coil, a, 10), try(pinball.pulse_coil, 1, a))
    out('lamp', i, try(pinball.set_lamp, 1, a), try(pinball.play_sound, 1, a), try(pinball.set_score, 1, a))
    out('random', i, try(pinball.random, a), try(pinball.random, 1, a), try(math.random, a))
    out('persist', i, try(pinball.persist_set, a, 1), try(pinball.persist_get, a), try(pinball.switch, a))
  end
  out('tick', try(pinball.tick), try(pinball.balls_in_play))
  out('names', try(function() return pinball.sw end), try(function() pinball.sw.foo = 1 end), try(rawset, pinball, 'x', 1))
  out('extra_args', try(pinball.pulse_coil, 1, 2, 3, 4), try(pinball.tick, 1))
end
