-- Spec 2.9 clean rules (Independent Tester).
-- Attract until `start` closes; then a 3-ball game: each ball is served by
-- dispenser `feed` (no plunger), a trough_1 closing edge ends the ball and
-- serves the next; after ball 3 the game is over (attract again; `start`
-- begins a new game). Scoring (player 1): +10 top_opto, +10 mid_opto,
-- +100 bumper ring (and the bumper coil fires). The first ring hit of a game
-- starts mode `frenzy` (show_scene(scene_frenzy)).
-- Ball search: when no ball is in play for 2000 ticks during a game (a ball
-- left the table without draining), the ball ends as if it had drained.
-- Persist (test observables): game_state 0 attract / 1 playing / 2 over,
-- ball_ticks = ticks of the current game with a ball in play,
-- games_done = games finished since load, balls_served.
local BALLS = 3
local state, ball, score, ball_ticks, games_done, served = 0, 0, 0, 0, 0, 0
local frenzy = false
local empty = 0
local dirty = true
local sw, coil, disp, prof, key, scene

local function serve()
  pinball.spawn_ball(disp.feed, prof.standard_steel)
  served = served + 1
end

function on_init()
  sw = pinball.sw
  coil = pinball.coil
  disp = pinball.dispenser
  prof = pinball.profile
  key = pinball.persist_key
  scene = pinball.scene
end

local function end_ball()
  if ball < BALLS then
    ball = ball + 1
    serve()
  else
    state = 2
    games_done = games_done + 1
  end
  dirty = true
end

local function start_mode()
  frenzy = true
  pinball.show_scene(scene.scene_frenzy)
  pinball.set_lamp(pinball.lamp.lamp_frenzy, 1.0)
end

function on_switch(id, closed)
  if not closed then return end
  if id == sw.start then
    if state ~= 1 then
      state, ball, score, ball_ticks, frenzy = 1, 1, 0, 0, false
      pinball.set_lamp(pinball.lamp.lamp_frenzy, 0.0)
      serve()
    end
  elseif state ~= 1 then
    return
  elseif id == sw.top_opto or id == sw.mid_opto then
    score = score + 10
  elseif id == sw.bumper_1_ring then
    score = score + 100
    pinball.pulse_coil(coil.bumper_1_coil, 20)
    if not frenzy then start_mode() end
  elseif id == sw.trough_1 then
    end_ball()
  else
    return
  end
  dirty = true
end

function on_tick()
  if state == 1 and pinball.balls_in_play() > 0 then
    ball_ticks = ball_ticks + 1
    empty = 0
  elseif state == 1 then
    empty = empty + 1
    if empty >= 2000 then
      empty = 0
      end_ball()
    end
  end
  if dirty or pinball.tick() % 100 == 0 then
    pinball.set_score(1, score)
    pinball.persist_set(key.game_state, state)
    pinball.persist_set(key.ball_ticks, ball_ticks)
    pinball.persist_set(key.games_done, games_done)
    pinball.persist_set(key.balls_served, served)
    dirty = false
  end
end
