-- Integer-only parity ruleset (Subphase 4.5). Its Rust twin is
-- parity_guest/src/lib.rs, compiled to rules.wasm; both must emit identical
-- HostCommandQueue contents on every tick for the same inputs.
local score = {0, 0, 0, 0}
local player = 1
local combo = 0
local held = false
local lamp_phase = 0
local hits = 0

function on_init()
  pinball.show_scene(1)
  pinball.persist_set(0, pinball.persist_get(0) + 1)
  pinball.spawn_ball(0, 0)
  pinball.set_gi_tint(1, 1, 1, 1)
end

function on_switch(id, closed)
  if closed then
    hits = hits + 1
    local pts = 10 * (id + 1) + pinball.random(0, 9)
    if combo > 0 then pts = pts * 2 end
    combo = 250
    score[player] = score[player] + pts
    pinball.set_score(player, score[player])
    if id % 4 == 0 then pinball.pulse_coil(id, 10 + id % 20) end
    if id % 5 == 1 then pinball.play_sound(id % 8, 1) end
    if id == 7 then
      held = not held
      pinball.hold_coil(2, held)
    end
    if hits % 25 == 0 then
      pinball.spawn_ball(hits % 3, pinball.random(1, 3))
      pinball.show_scene(2 + hits % 4)
    end
    if hits % 17 == 0 then
      player = player % 4 + 1
      pinball.set_diverter(1, player % 2 == 0)
    end
  else
    if id == 3 then pinball.set_zipper(0, false) end
    if id == 4 then pinball.set_zipper(0, true) end
  end
end

function on_tick()
  local t = pinball.tick()
  if combo > 0 then combo = combo - 1 end
  if t % 50 == 0 then
    lamp_phase = (lamp_phase + 1) % 8
    for i = 0, 7 do
      if i == lamp_phase then pinball.set_lamp(i, 1) else pinball.set_lamp(i, 0) end
    end
  end
  if t % 97 == 0 then
    local r = pinball.random(100)
    if r > 70 then pinball.play_sound(pinball.sound.jackpot, 1) end
    if r < 10 then pinball.set_magnet(0, 1) else pinball.set_magnet(0, 0) end
  end
  if t % 500 == 0 then
    local best = pinball.persist_get(1)
    if score[player] > best then pinball.persist_set(1, score[player]) end
    pinball.persist_set(2, pinball.persist_get(2) + pinball.balls_in_play())
  end
  if pinball.switch(5) and t % 10 == 0 then pinball.pulse_coil(5, 15) end
  if t % 333 == 0 then pinball.set_gi_tint(1, 0, t % 2, 1) end
end
