-- Spec fault-parity ruleset (subphase 4.5), Lua side. Holds 39 coils over two
-- ticks, then faults on an argument violation in tick 3 after queueing
-- commands. parity_fault.wat is the same ruleset for the Wasm ABI.
local n = 0
function on_tick()
  n = n + 1
  if n == 1 then
    for c = 100, 131 do pinball.hold_coil(c, true) end
  elseif n == 2 then
    for c = 200, 207 do pinball.hold_coil(c, true) end
    pinball.hold_coil(100, false)
  elseif n == 3 then
    pinball.pulse_coil(1, 5)
    pinball.hold_coil(300, true)
    pinball.set_lamp(1, 1)
    pinball.set_gi_tint(0.5, 0.5, 0.5, 0.5)
    pinball.pulse_coil(70000, 1) -- coil id out of range: faults
  else
    pinball.show_scene(1)
  end
end
function on_switch(id, closed)
  pinball.set_score(1, id)
end
