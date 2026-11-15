-- Subphase 2.9 autoplay fixture rules (trap). A three-ball game started
-- by `start`; each drain into the trough serves the next ball.
local sw = pinball.sw
local in_game = false
local balls_left = 0
local score = 0
local bumper_hits = 0

local function add(points)
  score = score + points
  pinball.set_score(1, score)
end

local function launch()
  pinball.spawn_ball(pinball.dispenser.trough_exit, pinball.profile.standard_steel)
end

-- A mode start: count it in the mode's `mode:<id>` persist key (what the
-- soak observes) and show its start scene.
local function start_mode(key, scene)
  pinball.persist_set(key, pinball.persist_get(key) + 1)
  pinball.show_scene(scene)
end

function on_init()
  pinball.set_score(1, 0)
end

function on_switch(id, closed)
  if not closed then return end
  if id == sw.start then
    if not in_game then
      in_game = true
      balls_left = 3
      bumper_hits = 0
      score = 0
      add(0)
      launch()
    end
  elseif id == sw.trough then
    if in_game then
      balls_left = balls_left - 1
      if balls_left > 0 then launch() else in_game = false end
    end
  end
end

function on_tick()
end
