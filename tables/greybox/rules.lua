-- Grey-box table rules (Subphase 6.0). Integer arithmetic only; the
-- callbacks create no Lua objects (no strings, tables or closures), so the
-- steady state performs no Lua allocation.
--
-- Game flow: attract -> START -> a 3-ball game -> game over -> attract.
-- A ball ends when it drains into a trough switch (trough_1 / trough_2).
-- Ball 2 of every game is the ceramic Powerball (dispenser `feed`). The
-- Powerball is detected at the magnet: the opto closes but the eddy sensor
-- (conductive balls only) does not; that starts the `powerball` mode
-- (declared in table.toml [[modes]]) for the rest of that ball.
--
-- Scoring (player 1): bumper 100, sling 10, orbit entry 500, orbit top 5000,
-- ramp entry 100, ramp made 10000, VUK 2500, upper deck 1500, eddy 250,
-- Newton ball 2000, drop target 750, flipper buttons 0. Scores double while
-- the powerball mode runs.
--
-- Persistent values (`[persist]` keys, readable by the host):
--   games_played, high_score, ball (ball in play, 0 when no game),
--   game_state (0 attract, 1 game, 2 game over), powerball_modes (starts).

local ATTRACT, GAME, OVER = 0, 1, 2
local BALLS_PER_GAME = 3
local POWERBALL_BALL = 2
-- Ticks after the opto closes within which the eddy sensor must have closed
-- for a conductive (steel) ball.
local DETECT_WINDOW = 30
local VUK_HOLD = 400
local DROP_RESET_DELAY = 1000
local MAGNET_PULSE = 120
local GAME_OVER_TICKS = 4000

local state = ATTRACT
local score, ball, games, high = 0, 0, 0, 0
local ticks = 0
local dirty = true
local pb_mode = false
local pb_starts = 0
local opto_tick, eddy_tick = -1000000, -1000000
local opto_pending = false
local vuk_at, drop_at, magnet_off_at, attract_at = -1, -1, -1, -1
local last_drain = -1

local sw, coil, scene, snd = {}, {}, {}, {}
local disp_feed, prof_steel, prof_power, mag, lamp_pb
local k_games, k_high, k_ball, k_state, k_pb

function on_init()
  local s = pinball.sw
  sw.start, sw.launch, sw.tilt = s.start, s.launch, s.tilt
  sw.trough_1, sw.trough_2 = s.trough_1, s.trough_2
  sw.b1, sw.b2, sw.b3 = s.bumper_1_ring, s.bumper_2_ring, s.bumper_3_ring
  sw.sl, sw.sr = s.sling_left_sw, s.sling_right_sw
  sw.orbit_entry, sw.orbit_top = s.orbit_entry, s.orbit_top
  sw.ramp_entry, sw.ramp_made = s.ramp_entry, s.ramp_made
  sw.vuk, sw.deck = s.vuk_sw, s.deck_opto
  sw.eddy, sw.opto = s.magnet_eddy, s.magnet_opto
  sw.newton, sw.drop = s.newton_sw, s.drop_1_sw
  local c = pinball.coil
  coil.b1, coil.b2, coil.b3 = c.bumper_1_coil, c.bumper_2_coil, c.bumper_3_coil
  coil.sl, coil.sr = c.sling_left_coil, c.sling_right_coil
  coil.vuk, coil.plunger, coil.drop = c.vuk_coil, c.plunger_coil, c.drop_reset_coil
  scene.attract = pinball.scene.scene_attract
  scene.score = pinball.scene.scene_score
  scene.over = pinball.scene.scene_game_over
  scene.pb = pinball.scene.scene_powerball_mania
  snd.bumper, snd.sling = pinball.sound.sfx_bumper, pinball.sound.sfx_sling
  disp_feed = pinball.dispenser.feed
  prof_steel = pinball.profile.standard_steel
  prof_power = pinball.profile.ceramic_powerball
  mag = pinball.magnet.lock_magnet
  lamp_pb = pinball.lamp.powerball_lamp
  local p = pinball.persist_key
  k_games, k_high, k_ball, k_state, k_pb =
    p.games_played, p.high_score, p.ball, p.game_state, p.powerball_modes
  games = pinball.persist_get(k_games)
  high = pinball.persist_get(k_high)
  pinball.persist_set(k_ball, 0)
  pinball.persist_set(k_state, ATTRACT)
  pinball.persist_set(k_pb, 0)
  pinball.show_scene(scene.attract)
end

local function add(points)
  if pb_mode then points = points * 2 end
  score = score + points
  dirty = true
end

local function serve_ball()
  if ball == POWERBALL_BALL then
    pinball.spawn_ball(disp_feed, prof_power)
  else
    pinball.spawn_ball(disp_feed, prof_steel)
  end
  pinball.persist_set(k_ball, ball)
end

local function start_game()
  state = GAME
  score, ball = 0, 1
  pb_mode = false
  attract_at = -1
  dirty = true
  pinball.persist_set(k_state, GAME)
  pinball.show_scene(scene.score)
  serve_ball()
end

local function end_ball()
  if pb_mode then
    pb_mode = false
    pinball.set_lamp(lamp_pb, 0)
  end
  if ball < BALLS_PER_GAME then
    ball = ball + 1
    serve_ball()
    return
  end
  state = OVER
  ball = 0
  games = games + 1
  if score > high then high = score end
  pinball.persist_set(k_games, games)
  pinball.persist_set(k_high, high)
  pinball.persist_set(k_ball, 0)
  pinball.persist_set(k_state, OVER)
  pinball.show_scene(scene.over)
  attract_at = ticks + GAME_OVER_TICKS
end

local function start_powerball()
  pb_mode = true
  pb_starts = pb_starts + 1
  pinball.persist_set(k_pb, pb_starts)
  pinball.set_lamp(lamp_pb, 1)
  pinball.show_scene(scene.pb)
end

function on_switch(id, closed)
  if not closed then return end
  if id == sw.start then
    if state ~= GAME then start_game() end
    return
  end
  if state ~= GAME then return end
  if id == sw.trough_1 or id == sw.trough_2 then
    -- One drain can close both trough switches in the same tick.
    if last_drain ~= ticks then
      last_drain = ticks
      end_ball()
    end
  elseif id == sw.launch then
    pinball.pulse_coil(coil.plunger, 300)
  elseif id == sw.b1 then
    pinball.pulse_coil(coil.b1, 20); pinball.play_sound(snd.bumper, 1); add(100)
  elseif id == sw.b2 then
    pinball.pulse_coil(coil.b2, 20); pinball.play_sound(snd.bumper, 1); add(100)
  elseif id == sw.b3 then
    pinball.pulse_coil(coil.b3, 20); pinball.play_sound(snd.bumper, 1); add(100)
  elseif id == sw.sl then
    pinball.pulse_coil(coil.sl, 25); pinball.play_sound(snd.sling, 1); add(10)
  elseif id == sw.sr then
    pinball.pulse_coil(coil.sr, 25); pinball.play_sound(snd.sling, 1); add(10)
  elseif id == sw.orbit_entry then
    add(500)
  elseif id == sw.orbit_top then
    add(5000)
  elseif id == sw.ramp_entry then
    add(100)
  elseif id == sw.ramp_made then
    add(10000)
  elseif id == sw.vuk then
    add(2500)
    vuk_at = ticks + VUK_HOLD
  elseif id == sw.deck then
    add(1500)
  elseif id == sw.eddy then
    eddy_tick = ticks
    pinball.set_magnet(mag, 1)
    magnet_off_at = ticks + MAGNET_PULSE
    add(250)
  elseif id == sw.opto then
    opto_tick = ticks
    opto_pending = true
  elseif id == sw.newton then
    add(2000)
  elseif id == sw.drop then
    add(750)
    drop_at = ticks + DROP_RESET_DELAY
  end
end

function on_tick()
  ticks = ticks + 1
  if opto_pending and ticks - opto_tick > DETECT_WINDOW then
    opto_pending = false
    -- No eddy edge around the opto edge: a non-conductive ball.
    if state == GAME and not pb_mode and eddy_tick < opto_tick - DETECT_WINDOW then
      start_powerball()
    end
  end
  if vuk_at >= 0 and ticks >= vuk_at then
    vuk_at = -1
    pinball.pulse_coil(coil.vuk, 20)
  end
  if drop_at >= 0 and ticks >= drop_at then
    drop_at = -1
    pinball.pulse_coil(coil.drop, 150)
  end
  if magnet_off_at >= 0 and ticks >= magnet_off_at then
    magnet_off_at = -1
    pinball.set_magnet(mag, 0)
  end
  if attract_at >= 0 and ticks >= attract_at then
    attract_at = -1
    if state == OVER then
      state = ATTRACT
      pinball.persist_set(k_state, ATTRACT)
      pinball.show_scene(scene.attract)
    end
  end
  if dirty then
    pinball.set_score(1, score)
    dirty = false
  end
end
