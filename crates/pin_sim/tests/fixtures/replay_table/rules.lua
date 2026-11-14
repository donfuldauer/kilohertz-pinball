-- Subphase 2.5 replay fixture rules: integer-only, allocation-free in steady
-- state (no tables or strings are created after load).
local score = 0
local t = 0
local blink = 0

local function add(points)
  score = score + points
  pinball.set_score(1, score)
end

local function launch()
  pinball.spawn_ball(pinball.dispenser.trough_exit, pinball.profile.standard_steel)
end

function on_init()
  pinball.set_score(1, 0)
  launch()
end

function on_switch(id, closed)
  if not closed then return end
  if id == pinball.sw.bumper_ring then
    pinball.pulse_coil(pinball.coil.bumper_coil, 20)
    pinball.set_lamp(pinball.lamp.bumper_lamp, 1)
    add(100)
  elseif id == pinball.sw.center_rollover then
    add(10 + pinball.random(0, 9))
  elseif id == pinball.sw.magnet_eddy then
    add(50)
  elseif id == pinball.sw.trough then
    add(1)
    launch()
  elseif id == pinball.sw.start then
    if pinball.balls_in_play() == 0 then launch() end
  elseif id == pinball.sw.left_flipper or id == pinball.sw.right_flipper then
    add(1)
  end
end

function on_tick()
  t = t + 1
  if t % 250 == 0 then
    blink = 1 - blink
    pinball.set_lamp(pinball.lamp.blink_lamp, blink)
    pinball.set_lamp(pinball.lamp.bumper_lamp, 0)
  end
  if t % 1000 == 0 then
    pinball.set_magnet(pinball.magnet.orbit_magnet, 1)
  elseif t % 1000 == 300 then
    pinball.set_magnet(pinball.magnet.orbit_magnet, 0)
  end
end
