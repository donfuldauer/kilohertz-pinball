-- Apex Circuit rules (Subphase 6.1, variant A). Integer arithmetic only (the
-- parity rule: no computed floats; lamp and magnet powers are the integer
-- literals 0 and 1). After on_init the
-- callbacks create no Lua objects (no strings, tables or closures), so the
-- steady state performs no Lua allocation. Kept simple and table-driven for
-- the Wasm port (Subphase 6.3).
--
-- Game flow: attract -> START -> a 3-ball game -> bonus after each ball ->
-- game over (high-score entry scene first when the stored high score is
-- beaten) -> attract. Each ball is served from the dispenser `feed` into
-- the shooter lane and launched with LAUNCH (the auto-plunger coil). Ball 2
-- of every game is the ceramic Powerball, balls 1 and 3 are steel. A ball
-- ends when it drains into `trough_1` / `trough_2`.
--
-- Features (README.md "Rules"):
-- * Skill shot: after a plunge (the ball closes `shooter_lane`), the first
--   top-lane rollover within 6 s scores 10,000 x ball number on the lit
--   lane (lane ((ball - 1) mod 3) + 1, so it rotates per ball;
--   `skill_shot_count` + 1) and 1,000 on the others.
--   Skill-shot and lane-in-window awards are fixed (not multiplied).
-- * Combo: a left-orbit pass (spinner) feeds the upper-left flipper; a shot
--   switch (orbit_top, right_orbit_entry, ramp_made, lock_scoop_sw, a drop
--   target) within 4 s of the feed or of the previous combo hit steps the
--   combo level 1 -> 2 -> ... -> 5 (multiplier 1x..5x on playfield scoring).
--   12 s without a combo hit drops one level; a drain resets it. The first
--   time a ball reaches 2x, the `combo_chain` mode starts.
-- * Powerball Mania: the magnet opto closes with no eddy-sensor edge within
--   30 ms (a non-conductive ball: the Powerball) -> `powerball_mania`, x2
--   scoring until the ball drains. Total multiplier capped at 10.
-- * Overdrive: 3 ramps or 5 combo steps on a ball light Overdrive Ready.
--   Then ACTION fires Firewall Save (both lower flippers zip closed for
--   exactly 5000 ticks) when one ball is in play and an inlane rollover
--   closed in the last 3 s, else cashes the Apex Jackpot (50,000 minus
--   1,000 per full 100 ms since Overdrive was lit, at least 5,000). Without
--   Overdrive Ready, ACTION does nothing. Overdrive is consumed on use and
--   lost at the drain.
-- * Bonus at the drain: (ramps + left orbits + right orbits + drop targets +
--   scoops + top lanes of the ball) x 1,000 x the combo level.
--
-- Persistent values (`[persist]` keys, readable by the host): ball (0 when
-- no game), game_state (0 attract, 1 game, 2 game over), high_score,
-- games_played, firewall_save_count, apex_jackpot_count, skill_shot_count,
-- mode:powerball_mania and mode:combo_chain (start counts: the soak's
-- `mode:<id>` signal), combo_level (1..5), overdrive_ready (0/1),
-- last_bonus (the last end-of-ball bonus).
--
-- Hot reload: as the grey box, ids bind lazily (bind_ids) and every
-- rule-state local round-trips through on_save_state / on_restore_state.

local ATTRACT, GAME, OVER = 0, 1, 2
local BALLS_PER_GAME = 3
local POWERBALL_BALL = 2
local DETECT_WINDOW = 30
local MAGNET_PULSE = 40
local SKILL_WINDOW = 6000
local SKILL_VALUE = 10000
local LANE_VALUE = 1000
local COMBO_WINDOW = 4000
local COMBO_DECAY = 12000
local COMBO_MAX = 5
local MULT_CAP = 10
local OD_RAMPS = 3
local OD_STEPS = 5
local INLANE_WINDOW = 3000
local ZIPPER_TICKS = 5000
local JACKPOT_START = 50000
local JACKPOT_STEP = 1000
local JACKPOT_STEP_TICKS = 100
local JACKPOT_FLOOR = 5000
local BONUS_UNIT = 1000
local BONUS_TICKS = 2000
local HIGH_SCORE_TICKS = 3000
local GAME_OVER_TICKS = 4000
local SCOOP_HOLD = 1000
local DROP_RESET_DELAY = 1000
local NEVER = -1000000

-- Points per switch (multiplied).
local P_BUMPER, P_SLING, P_SPINNER = 100, 10, 50
local P_ORBIT_TOP, P_RO_ENTRY, P_MAGNET = 2000, 1000, 2500
local P_RAMP_ENTRY, P_RAMP_MADE = 500, 10000
local P_DROP, P_BANK = 750, 10000
local P_LOCK_LANE, P_SCOOP = 100, 5000
local P_INLANE = 100

local state = ATTRACT
local score, ball, games, high = 0, 0, 0, 0
local ticks = 0
local dirty = true
local ball_live = false
-- Per ball.
local pb_mode = false
local combo, combo_steps, chain_started = 1, 0, false
local feed_tick, combo_tick, decay_tick = NEVER, NEVER, NEVER
local ramps_ball, od_ramps, od_steps = 0, 0, 0
local od_ready, od_armed_tick = false, NEVER
local counters = 0
local skill_open, launch_tick, lit_lane = false, NEVER, 1
local inlane_tick = NEVER
local d1, d2, d3 = false, false, false
-- Detector and timers.
local opto_tick, eddy_tick, opto_pending = NEVER, NEVER, false
local magnet_off_at, scoop_at, drop_at = -1, -1, -1
local zipper_off_at, end_at, over_at, attract_at = -1, -1, -1, -1
-- Totals (persisted).
local pb_starts, chain_starts = 0, 0
local fw_count, jp_count, skill_count = 0, 0, 0

local sw, coil, scene, snd, lamp, key = {}, {}, {}, {}, {}, {}
local lane_sw, lane_lamp = {}, {}
local written = {}
local disp_feed, prof_steel, prof_power, mag, fl_left, fl_right

-- Fills the id tables from the read-only `pinball` name tables. Performs no
-- action, so it is safe from the reload hooks. Cheap no-op once bound.
local function bind_ids()
  if sw.start ~= nil then return end
  local s = pinball.sw
  sw.start, sw.launch, sw.action = s.start, s.launch, s.action
  sw.trough_1, sw.trough_2 = s.trough_1, s.trough_2
  sw.b1, sw.b2, sw.b3 = s.bumper_1_ring, s.bumper_2_ring, s.bumper_3_ring
  sw.sl, sw.sr = s.sling_left_sw, s.sling_right_sw
  sw.spinner, sw.orbit_top = s.left_spinner, s.orbit_top
  sw.ramp_entry, sw.ramp_made = s.ramp_entry, s.ramp_made
  sw.scoop, sw.lock_lane = s.lock_scoop_sw, s.lock_lane
  sw.eddy, sw.opto = s.magnet_eddy, s.magnet_opto
  sw.d1, sw.d2, sw.d3 = s.drop_1_sw, s.drop_2_sw, s.drop_3_sw
  sw.ro_entry, sw.shooter_lane = s.right_orbit_entry, s.shooter_lane
  sw.left_inlane, sw.right_inlane = s.left_inlane, s.right_inlane
  lane_sw[1], lane_sw[2], lane_sw[3] = s.lane_1, s.lane_2, s.lane_3
  local c = pinball.coil
  coil.b1, coil.b2, coil.b3 = c.bumper_1_coil, c.bumper_2_coil, c.bumper_3_coil
  coil.sl, coil.sr = c.sling_left_coil, c.sling_right_coil
  coil.scoop, coil.plunger, coil.drop = c.lock_scoop_coil, c.plunger_coil, c.drop_reset_coil
  local sc = pinball.scene
  scene.attract, scene.score, scene.over = sc.scene_attract, sc.scene_score, sc.scene_game_over
  scene.pb, scene.chain, scene.od = sc.scene_powerball_mania, sc.scene_combo_chain, sc.scene_overdrive
  scene.bonus, scene.high, scene.skill = sc.scene_bonus, sc.scene_high_score, sc.scene_skill_shot
  scene.fw, scene.jp = sc.scene_firewall_save, sc.scene_apex_jackpot
  snd.bumper, snd.sling = pinball.sound.sfx_bumper, pinball.sound.sfx_sling
  local l = pinball.lamp
  lamp.pb, lamp.od, lamp.combo = l.powerball_lamp, l.overdrive_lamp, l.combo_lamp
  lane_lamp[1], lane_lamp[2], lane_lamp[3] = l.lane_1_lamp, l.lane_2_lamp, l.lane_3_lamp
  disp_feed = pinball.dispenser.feed
  prof_steel = pinball.profile.standard_steel
  prof_power = pinball.profile.ceramic_powerball
  mag = pinball.magnet.apex_magnet
  fl_left, fl_right = pinball.flipper.left_flipper, pinball.flipper.right_flipper
  local p = pinball.persist_key
  key.ball, key.state, key.high, key.games = p.ball, p.game_state, p.high_score, p.games_played
  key.fw, key.jp, key.skill = p.firewall_save_count, p.apex_jackpot_count, p.skill_shot_count
  key.pb, key.chain = p["mode:powerball_mania"], p["mode:combo_chain"]
  key.combo, key.od, key.bonus = p.combo_level, p.overdrive_ready, p.last_bonus
  -- One cache slot per key (filled here, so later writes add no entry).
  for _, k in pairs(key) do written[k] = NEVER end
end

-- Writes a persist key only when its value changes (the host queue takes 8
-- persist writes per tick).
local function pset(k, v)
  if written[k] ~= v then
    written[k] = v
    pinball.persist_set(k, v)
  end
end

function on_init()
  bind_ids()
  games = pinball.persist_get(key.games)
  high = pinball.persist_get(key.high)
  fw_count = pinball.persist_get(key.fw)
  jp_count = pinball.persist_get(key.jp)
  skill_count = pinball.persist_get(key.skill)
  pb_starts = pinball.persist_get(key.pb)
  chain_starts = pinball.persist_get(key.chain)
  for _, k in pairs(key) do written[k] = pinball.persist_get(k) end
  pset(key.ball, 0)
  pset(key.state, ATTRACT)
  pset(key.combo, 1)
  pset(key.od, 0)
  pinball.show_scene(scene.attract)
end

-- Plain-data snapshot of every rule-state local (reload contract).
function on_save_state()
  return {
    state = state, score = score, ball = ball, games = games, high = high,
    ticks = ticks, ball_live = ball_live, pb_mode = pb_mode, combo = combo,
    combo_steps = combo_steps, chain_started = chain_started,
    feed_tick = feed_tick, combo_tick = combo_tick, decay_tick = decay_tick,
    ramps_ball = ramps_ball, od_ramps = od_ramps, od_steps = od_steps,
    od_ready = od_ready, od_armed_tick = od_armed_tick, counters = counters,
    skill_open = skill_open, launch_tick = launch_tick, lit_lane = lit_lane,
    inlane_tick = inlane_tick, d1 = d1, d2 = d2, d3 = d3,
    opto_tick = opto_tick, eddy_tick = eddy_tick, opto_pending = opto_pending,
    magnet_off_at = magnet_off_at, scoop_at = scoop_at, drop_at = drop_at,
    zipper_off_at = zipper_off_at, end_at = end_at, over_at = over_at,
    attract_at = attract_at, pb_starts = pb_starts, chain_starts = chain_starts,
    fw_count = fw_count, jp_count = jp_count, skill_count = skill_count,
  }
end

function on_restore_state(v)
  bind_ids()
  if type(v) ~= "table" then return end
  state, score, ball, games, high = v.state, v.score, v.ball, v.games, v.high
  ticks, ball_live, pb_mode, combo = v.ticks, v.ball_live, v.pb_mode, v.combo
  combo_steps, chain_started = v.combo_steps, v.chain_started
  feed_tick, combo_tick, decay_tick = v.feed_tick, v.combo_tick, v.decay_tick
  ramps_ball, od_ramps, od_steps = v.ramps_ball, v.od_ramps, v.od_steps
  od_ready, od_armed_tick, counters = v.od_ready, v.od_armed_tick, v.counters
  skill_open, launch_tick, lit_lane = v.skill_open, v.launch_tick, v.lit_lane
  inlane_tick, d1, d2, d3 = v.inlane_tick, v.d1, v.d2, v.d3
  opto_tick, eddy_tick, opto_pending = v.opto_tick, v.eddy_tick, v.opto_pending
  magnet_off_at, scoop_at, drop_at = v.magnet_off_at, v.scoop_at, v.drop_at
  zipper_off_at, end_at, over_at = v.zipper_off_at, v.end_at, v.over_at
  attract_at, pb_starts, chain_starts = v.attract_at, v.pb_starts, v.chain_starts
  fw_count, jp_count, skill_count = v.fw_count, v.jp_count, v.skill_count
  dirty = true
end

-- The playfield multiplier: combo level, doubled in Powerball Mania, capped.
local function multiplier()
  local m = combo
  if pb_mode then m = m * 2 end
  if m > MULT_CAP then m = MULT_CAP end
  return m
end

-- Playfield scoring (multiplied); only while a ball is in play.
local function add(points)
  if state ~= GAME or not ball_live then return end
  score = score + points * multiplier()
  dirty = true
end

-- Fixed awards (skill shot, jackpot, bonus): not multiplied.
local function award(points)
  score = score + points
  dirty = true
end

local function set_lane_lamps()
  for i = 1, 3 do
    if skill_open and i == lit_lane then
      pinball.set_lamp(lane_lamp[i], 1)
    else
      pinball.set_lamp(lane_lamp[i], 0)
    end
  end
end

local function arm_overdrive()
  if od_ready then return end
  od_ready = true
  od_armed_tick = ticks
  od_ramps, od_steps = 0, 0
  pinball.set_lamp(lamp.od, 1)
  pset(key.od, 1)
  pinball.show_scene(scene.od)
end

local function clear_overdrive()
  od_ready = false
  pinball.set_lamp(lamp.od, 0)
  pset(key.od, 0)
end

local function start_chain()
  chain_started = true
  chain_starts = chain_starts + 1
  pset(key.chain, chain_starts)
  pinball.show_scene(scene.chain)
end

local function combo_step()
  combo_steps = combo_steps + 1
  combo_tick = ticks
  decay_tick = ticks
  if combo < COMBO_MAX then
    combo = combo + 1
    pset(key.combo, combo)
    pinball.set_lamp(lamp.combo, 1)
  end
  if combo >= 2 and not chain_started then start_chain() end
  od_steps = od_steps + 1
  if od_steps >= OD_STEPS then arm_overdrive() end
end

-- A shot switch: counts for the ball's bonus and continues a combo when it
-- comes within the window of the feed or of the previous combo hit.
local function shot_hit()
  counters = counters + 1
  local since = feed_tick
  if combo_tick > since then since = combo_tick end
  if since > NEVER and ticks - since <= COMBO_WINDOW then combo_step() end
end

local function start_powerball()
  pb_mode = true
  pb_starts = pb_starts + 1
  pset(key.pb, pb_starts)
  pinball.set_lamp(lamp.pb, 1)
  pinball.show_scene(scene.pb)
end

local function reset_ball_state()
  pb_mode = false
  combo, combo_steps, chain_started = 1, 0, false
  feed_tick, combo_tick, decay_tick = NEVER, NEVER, NEVER
  ramps_ball, od_ramps, od_steps = 0, 0, 0
  counters = 0
  skill_open, launch_tick = false, NEVER
  inlane_tick = NEVER
  opto_pending = false
  pset(key.combo, 1)
  pinball.set_lamp(lamp.pb, 0)
  pinball.set_lamp(lamp.combo, 0)
  if od_ready then clear_overdrive() end
end

local function serve_ball()
  reset_ball_state()
  lit_lane = ((ball - 1) % 3) + 1
  ball_live = true
  if d1 or d2 or d3 then drop_at = ticks end
  if ball == POWERBALL_BALL then
    pinball.spawn_ball(disp_feed, prof_power)
  else
    pinball.spawn_ball(disp_feed, prof_steel)
  end
  pset(key.ball, ball)
  set_lane_lamps()
end

local function start_game()
  state = GAME
  score, ball = 0, 1
  end_at, over_at, attract_at = -1, -1, -1
  dirty = true
  pset(key.state, GAME)
  pinball.show_scene(scene.score)
  serve_ball()
end

local function drain()
  ball_live = false
  opto_pending = false
  local bonus = counters * BONUS_UNIT * combo
  award(bonus)
  pset(key.bonus, bonus)
  pinball.show_scene(scene.bonus)
  -- Mania and the combo end with the ball.
  pb_mode = false
  combo = 1
  pset(key.combo, 1)
  pinball.set_lamp(lamp.pb, 0)
  pinball.set_lamp(lamp.combo, 0)
  if od_ready then clear_overdrive() end
  skill_open = false
  set_lane_lamps()
  end_at = ticks + BONUS_TICKS
end

local function game_over()
  state = OVER
  ball = 0
  games = games + 1
  pset(key.games, games)
  pset(key.ball, 0)
  pset(key.state, OVER)
  if score > high then
    high = score
    pset(key.high, high)
    pinball.show_scene(scene.high)
    over_at = ticks + HIGH_SCORE_TICKS
  else
    pinball.show_scene(scene.over)
    attract_at = ticks + GAME_OVER_TICKS
  end
end

local function next_ball()
  if ball < BALLS_PER_GAME then
    ball = ball + 1
    pinball.show_scene(scene.score)
    serve_ball()
  else
    game_over()
  end
end

local function lane(i)
  counters = counters + 1
  if skill_open and ticks - launch_tick <= SKILL_WINDOW then
    skill_open = false
    if i == lit_lane then
      award(SKILL_VALUE * ball)
      skill_count = skill_count + 1
      pset(key.skill, skill_count)
      pinball.show_scene(scene.skill)
    else
      award(LANE_VALUE)
    end
    set_lane_lamps()
  else
    add(LANE_VALUE)
  end
end

local function action()
  if state ~= GAME or not ball_live or not od_ready then return end
  clear_overdrive()
  if pinball.balls_in_play() == 1 and ticks - inlane_tick <= INLANE_WINDOW then
    pinball.set_zipper(fl_left, true)
    pinball.set_zipper(fl_right, true)
    zipper_off_at = ticks + ZIPPER_TICKS
    fw_count = fw_count + 1
    pset(key.fw, fw_count)
    pinball.show_scene(scene.fw)
  else
    local v = JACKPOT_START - JACKPOT_STEP * ((ticks - od_armed_tick) // JACKPOT_STEP_TICKS)
    if v < JACKPOT_FLOOR then v = JACKPOT_FLOOR end
    award(v)
    jp_count = jp_count + 1
    pset(key.jp, jp_count)
    pinball.show_scene(scene.jp)
  end
end

-- The bank is complete when all three targets are down: reset it 1 s later.
local function bank_check()
  if d1 and d2 and d3 then
    add(P_BANK)
    drop_at = ticks + DROP_RESET_DELAY
  end
end

function on_switch(id, closed)
  bind_ids()
  -- Drop-target state follows both edges (a target is down while closed).
  if id == sw.d1 then d1 = closed
  elseif id == sw.d2 then d2 = closed
  elseif id == sw.d3 then d3 = closed end
  if not closed then return end
  -- Mechanisms work in every state (an injected ball is never left held).
  if id == sw.b1 then
    pinball.pulse_coil(coil.b1, 20); pinball.play_sound(snd.bumper, 1); add(P_BUMPER)
  elseif id == sw.b2 then
    pinball.pulse_coil(coil.b2, 20); pinball.play_sound(snd.bumper, 1); add(P_BUMPER)
  elseif id == sw.b3 then
    pinball.pulse_coil(coil.b3, 20); pinball.play_sound(snd.bumper, 1); add(P_BUMPER)
  elseif id == sw.sl then
    pinball.pulse_coil(coil.sl, 25); pinball.play_sound(snd.sling, 1); add(P_SLING)
  elseif id == sw.sr then
    pinball.pulse_coil(coil.sr, 25); pinball.play_sound(snd.sling, 1); add(P_SLING)
  elseif id == sw.scoop then
    scoop_at = ticks + SCOOP_HOLD
    add(P_SCOOP)
    if state == GAME and ball_live then shot_hit() end
  elseif id == sw.launch then
    pinball.pulse_coil(coil.plunger, 300)
  elseif id == sw.start then
    if state ~= GAME then start_game() end
  elseif id == sw.eddy then
    eddy_tick = ticks
    pinball.set_magnet(mag, 1)
    magnet_off_at = ticks + MAGNET_PULSE
  elseif id == sw.opto then
    opto_tick = ticks
    opto_pending = true
    add(P_MAGNET)
  end
  if state ~= GAME or not ball_live then return end
  if id == sw.trough_1 or id == sw.trough_2 then
    drain()
  elseif id == sw.action then
    action()
  elseif id == sw.spinner then
    feed_tick = ticks
    add(P_SPINNER)
  elseif id == sw.orbit_top then
    add(P_ORBIT_TOP); shot_hit()
  elseif id == sw.ro_entry then
    add(P_RO_ENTRY); shot_hit()
  elseif id == sw.ramp_entry then
    add(P_RAMP_ENTRY)
  elseif id == sw.ramp_made then
    add(P_RAMP_MADE)
    ramps_ball = ramps_ball + 1
    od_ramps = od_ramps + 1
    shot_hit()
    if od_ramps >= OD_RAMPS then arm_overdrive() end
  elseif id == sw.d1 or id == sw.d2 or id == sw.d3 then
    add(P_DROP); shot_hit()
    bank_check()
  elseif id == sw.lock_lane then
    add(P_LOCK_LANE)
  elseif id == lane_sw[1] then lane(1)
  elseif id == lane_sw[2] then lane(2)
  elseif id == lane_sw[3] then lane(3)
  elseif id == sw.shooter_lane then
    -- A plunge: the ball runs up the shooter lane. Opens the skill shot.
    skill_open = true
    launch_tick = ticks
    set_lane_lamps()
  elseif id == sw.left_inlane or id == sw.right_inlane then
    inlane_tick = ticks
    add(P_INLANE)
  end
end

function on_tick()
  bind_ids()
  ticks = ticks + 1
  if opto_pending and ticks - opto_tick > DETECT_WINDOW then
    opto_pending = false
    -- No eddy edge around the opto edge: a non-conductive ball.
    if state == GAME and ball_live and not pb_mode
        and eddy_tick < opto_tick - DETECT_WINDOW then
      start_powerball()
    end
  end
  if magnet_off_at >= 0 and ticks >= magnet_off_at then
    magnet_off_at = -1
    pinball.set_magnet(mag, 0)
  end
  if scoop_at >= 0 and ticks >= scoop_at then
    scoop_at = -1
    pinball.pulse_coil(coil.scoop, 20)
  end
  if drop_at >= 0 and ticks >= drop_at then
    drop_at = -1
    pinball.pulse_coil(coil.drop, 150)
  end
  if zipper_off_at >= 0 and ticks >= zipper_off_at then
    zipper_off_at = -1
    pinball.set_zipper(fl_left, false)
    pinball.set_zipper(fl_right, false)
  end
  if combo > 1 and ticks - decay_tick >= COMBO_DECAY then
    combo = combo - 1
    decay_tick = ticks
    pset(key.combo, combo)
    if combo == 1 then pinball.set_lamp(lamp.combo, 0) end
  end
  if skill_open and ticks - launch_tick > SKILL_WINDOW then
    skill_open = false
    set_lane_lamps()
  end
  if end_at >= 0 and ticks >= end_at then
    end_at = -1
    if state == GAME then next_ball() end
  end
  if over_at >= 0 and ticks >= over_at then
    over_at = -1
    if state == OVER then
      pinball.show_scene(scene.over)
      attract_at = ticks + GAME_OVER_TICKS
    end
  end
  if attract_at >= 0 and ticks >= attract_at then
    attract_at = -1
    if state == OVER then
      state = ATTRACT
      pset(key.state, ATTRACT)
      pinball.show_scene(scene.attract)
    end
  end
  if dirty then
    pinball.set_score(1, score)
    dirty = false
  end
end
