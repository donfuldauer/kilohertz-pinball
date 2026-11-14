-- Spec fixture: a miniature rule set driven by scripted switch sequences.
local p = pinball
local score = 0

function on_init()
  p.show_scene(p.scene.attract)
  p.set_gi_tint(1, 1, 1, 0.5)
end

function on_switch(id, closed)
  if math.type(id) ~= "integer" or type(closed) ~= "boolean" then
    error("on_switch must receive (integer, boolean)")
  end
  if id == p.sw.start then
    if closed then
      score = 0
      p.spawn_ball(p.dispenser.trough, p.profile.steel)
      p.pulse_coil(p.coil.trough_eject, 40)
      p.show_scene(p.scene.multiball)
      p.set_score(1, score)
    end
  elseif id == p.sw.left_sling then
    if closed then
      score = score + 10
      p.pulse_coil(p.coil.left_sling, 25)
      p.play_sound(p.sound.bell, 0.5)
      p.set_score(1, score)
    else
      p.set_lamp(p.lamp.jackpot, 0)
    end
  elseif id == p.sw.spinner then
    -- both edges score
    score = score + 1
    p.set_score(1, score)
  elseif id == p.sw.trough_1 and closed then
    p.persist_set(p.persist_key.high_score_1, score)
    p.play_sound(p.sound.drain, 1)
    p.hold_coil(p.coil.knocker, p.switch(p.sw.start))
  end
end

function on_tick()
  if p.tick() % 4 == 0 then
    p.set_lamp(p.lamp.shoot_again, 1)
  end
end
