-- Spec 2.5 fixture rules (Independent Tester). Integer arithmetic only; the
-- callbacks create no Lua objects (no strings, tables or closures), so the
-- steady state performs no Lua allocation.
--
-- Reported scores (Lua player 1..4 = host player 0..3 = Simulation::score(0..3)):
--   1: game score: +10 lane_opto, +5 side_opto, +100 bumper ring (and the
--      bumper coil is pulsed), +1000 trough_1, +1 each flipper button press
--   2: number of on_tick calls completed when the last lane_opto closed edge
--      was delivered (-1 before the first)
--   3: number of on_tick calls completed when the last `action` edge (closed
--      or opened) was delivered (-1 before the first)
--   4: sum of pinball.random(1000000) drawn on each `start` press
local ticks = 0
local score = 0
local lane_at = -1
local action_at = -1
local rng_sum = 0
local dirty = true
local sw_lane, sw_side, sw_trough, sw_ring = -1, -1, -1, -1
local sw_start, sw_action, sw_lf, sw_rf = -1, -1, -1, -1
local coil_bumper, disp_feed, prof_steel = -1, -1, -1

function on_init()
  sw_lane = pinball.sw.lane_opto
  sw_side = pinball.sw.side_opto
  sw_trough = pinball.sw.trough_1
  sw_ring = pinball.sw.bumper_1_ring
  sw_start = pinball.sw.start
  sw_action = pinball.sw.action
  sw_lf = pinball.sw.left_flipper
  sw_rf = pinball.sw.right_flipper
  coil_bumper = pinball.coil.bumper_1_coil
  disp_feed = pinball.dispenser.feed
  prof_steel = pinball.profile.standard_steel
end

function on_switch(id, closed)
  if id == sw_action then
    action_at = ticks
    dirty = true
    return
  end
  if not closed then return end
  if id == sw_start then
    pinball.spawn_ball(disp_feed, prof_steel)
    rng_sum = rng_sum + pinball.random(1000000)
  elseif id == sw_lane then
    score = score + 10
    lane_at = ticks
  elseif id == sw_side then
    score = score + 5
  elseif id == sw_ring then
    score = score + 100
    pinball.pulse_coil(coil_bumper, 20)
  elseif id == sw_trough then
    score = score + 1000
  elseif id == sw_lf or id == sw_rf then
    score = score + 1
  else
    return
  end
  dirty = true
end

function on_tick()
  if dirty then
    pinball.set_score(1, score)
    pinball.set_score(2, lane_at)
    pinball.set_score(3, action_at)
    pinball.set_score(4, rng_sum)
    dirty = false
  end
  ticks = ticks + 1
end
