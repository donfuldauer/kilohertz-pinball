-- Feature Lab rules (Subphase 6.4). Integer arithmetic only; the callbacks
-- create no Lua objects (no strings, tables or closures), so the steady
-- state performs no Lua allocation. Time is the sim tick (pinball.tick(),
-- 1 ms); the only randomness is pinball.random.
--
-- Game flow (as the grey-box): attract -> START -> a 3-ball game -> game
-- over -> attract 4 s later. The player's ball is served into the shooter
-- lane (dispenser `feed`). A ball ends when the last ball in play has
-- drained through a trough switch (main trough_1 / trough_2, or the XL
-- area's xl_trough): extra balls (XL, Glo-Ball, comparison balls) never
-- end the player's ball by themselves.
--
-- Mode selector `mode_sel` (0..4, persist key): advances by one (mod 5) at
-- the start of every ball and on every ACTION press during a game. It is
-- drawn at random when the table loads and is not reset between games, so
-- autoplay (which never presses ACTION) cycles through all five modes
-- across games. The selected mode starts when its qualifying shot (the
-- [[shots]] entry of the same index, a two-switch sequence within its
-- window, matched as the soak monitor matches it) is made; each mode starts
-- at most once per ball. Each start increments the persist key
-- `mode:<id>` once, shows the mode's scene and scores MODE_START_POINTS.
--
--   0 banana_shots   shot banana_vuk (vuk_lane, bb_vuk_sw), the Banana's
--                    shot up the VUK lane: further VUK lane shots score
--                    25000 for the rest of the ball.
--   1 backbox_play   shot backbox_run (bb_vuk_sw, bb_drain), a ball
--                    through the backbox: the backbox drain (bb_drain)
--                    scores 5000 instead of 500 and the backbox flipper
--                    1000 for the rest of the ball.
--   2 xl_ball        shot pops_down (bumper_3_ring, bumper_2_ring): the XL ball is
--                    served into the XL area (xl_feed); xl_lane scores 500;
--                    the mode ends when xl_trough drains it or after 20 s.
--   3 magnet_compare shot magnet_lane (magnet_opto, vuk_lane): the
--                    magnet_feed serves a standard_steel ball onto the
--                    energised holding magnet, then (1.5 s after it left) a
--                    low_rem_steel ball; the magnet holds each for 1 s; the
--                    rules measure the time from the coil going off
--                    to the ball leaving the pole opto (persist keys
--                    cmp_release_ms_standard / cmp_release_ms_low_rem).
--   4 glo_ball       shot pops (bumper_2_ring, bumper_3_ring): the Glo-Ball
--                    is served into the shooter lane (glo_feed) and plunged
--                    round the orbit 1.5 s later; bumpers score double; the
--                    mode ends after 30 s or with the ball.
--
-- Scoring (player 1): bumper 10 (20 in glo_ball), sling 10, orbit entry 100, orbit top
-- 5000 when made from the bottom (orbit_entry within 3 s before), else 500;
-- a flipper shot (orbit entry, VUK lane, magnet opto or a bumper within 1 s
-- of a lower flipper reaching its end of stroke) 5000, once per flip; VUK
-- lane 100, backbox VUK 500, backbox drain 500 (5000 in backbox_play),
-- magnet opto 100, XL lane 500, mode start 2000, comparison measured
-- 10000.
--
-- Persistent values (`[persist]` keys, readable by the host): games_played,
-- high_score, ball (1..3 in a game, else 0), game_state (0 attract, 1 game,
-- 2 game over), mode_sel, mode:<id> (start counts), cmp_release_ms_standard,
-- cmp_release_ms_low_rem (last measured release time, ms; 0 until
-- measured).
--
-- Hot reload: a reload runs this file in a fresh state and does not call
-- on_init again; the old on_save_state() result is handed to the new
-- on_restore_state(). The id tables are therefore bound lazily (bind_ids,
-- name lookups only, no actions) and every rule-state local round-trips
-- through on_save_state / on_restore_state as plain data.

local ATTRACT, GAME, OVER = 0, 1, 2
local BALLS_PER_GAME = 3
local MODES = 5
local MODE_START_POINTS = 2000
local VUK_HOLD = 400
local GAME_OVER_TICKS = 4000
local XL_MODE_TICKS = 20000
local GLO_MODE_TICKS = 30000
local GLO_LAUNCH_DELAY = 1500
-- A target hit within FLIP_SHOT_TICKS of a lower flipper reaching its end
-- of stroke (its EOS switch opening) is a flipper shot: FLIP_SHOT_POINTS.
local FLIP_SHOT_TICKS = 1000
local FLIP_SHOT_POINTS = 5000
-- The orbit made from the bottom (orbit_entry, then orbit_top within
-- ORBIT_TICKS) scores ORBIT_POINTS; orbit_top alone (every plunge) 500.
local ORBIT_TICKS = 3000
local ORBIT_POINTS = 5000
-- Magnet comparison: phases and times (ticks).
local CMP_IDLE, CMP_WAIT, CMP_HOLD, CMP_RELEASE, CMP_GAP = 0, 1, 2, 3, 4
local CMP_HOLD_TICKS = 1000
local CMP_TIMEOUT = 3000
-- The first comparison ball rolls well clear of the pole before the second
-- is served there.
local CMP_GAP_TICKS = 1500

local state = ATTRACT
local score, ball, games, high = 0, 0, 0, 0
local ticks = 0
local dirty = true
local mode_sel = 0
local last_spawn = -1000000
local vuk_at, attract_at = -1, -1
local last_flip, last_entry = -1000000, -1000000
-- Per mode (index 1..5 = selector 0..4): started this ball, start counts.
local started = { false, false, false, false, false }
local starts = { 0, 0, 0, 0, 0 }
-- Shot progress (index 1..5): 1 after the first switch, at shot_start.
local shot_next = { 0, 0, 0, 0, 0 }
local shot_start = { 0, 0, 0, 0, 0 }
local banana_on, backbox_on, xl_on, glo_on = false, false, false, false
local xl_end, glo_end, glo_launch = -1, -1, -1
local cmp_phase, cmp_which, cmp_deadline, cmp_hold_until, cmp_off = 0, 0, -1, -1, -1

local sw, coil, scene, snd = {}, {}, {}, {}
local shot_a, shot_b, shot_win = {}, {}, {}
local mode_scene, mode_key = {}, {}
local disp, prof = {}, {}
local mag, lamp_mode
local k_games, k_high, k_ball, k_state, k_sel, k_cmp_std, k_cmp_low

-- Fills the id tables from the read-only `pinball` name tables. Performs no
-- action (no scene, persist write, coil or sound), so it is safe from the
-- reload hooks. Cheap no-op once bound.
local function bind_ids()
  if sw.start ~= nil then return end
  local s = pinball.sw
  sw.start, sw.launch, sw.action = s.start, s.launch, s.action
  sw.trough_1, sw.trough_2, sw.xl_trough = s.trough_1, s.trough_2, s.xl_trough
  sw.b1, sw.b2, sw.b3 = s.bumper_1_ring, s.bumper_2_ring, s.bumper_3_ring
  sw.sl = s.sling_left_sw
  sw.orbit_entry, sw.orbit_top = s.orbit_entry, s.orbit_top
  sw.vuk_lane, sw.bb_vuk = s.vuk_lane, s.bb_vuk_sw
  sw.magnet = s.magnet_opto
  sw.xl_lane, sw.bb_eos, sw.bb_drain = s.xl_lane, s.bb_flipper_eos, s.bb_drain
  sw.plunger_tip = s.plunger_tip
  sw.ll_eos, sw.lr_eos = s.lower_left_eos, s.lower_right_eos
  -- The qualifying shots, in selector order (table.toml [[shots]]).
  shot_a[1], shot_b[1], shot_win[1] = s.vuk_lane, s.bb_vuk_sw, 3000
  shot_a[2], shot_b[2], shot_win[2] = s.bb_vuk_sw, s.bb_drain, 10000
  shot_a[3], shot_b[3], shot_win[3] = s.bumper_3_ring, s.bumper_2_ring, 5000
  shot_a[4], shot_b[4], shot_win[4] = s.magnet_opto, s.vuk_lane, 2000
  shot_a[5], shot_b[5], shot_win[5] = s.bumper_2_ring, s.bumper_3_ring, 5000
  local c = pinball.coil
  coil.b1, coil.b2, coil.b3 = c.bumper_1_coil, c.bumper_2_coil, c.bumper_3_coil
  coil.sl = c.sling_left_coil
  coil.vuk, coil.plunger = c.bb_vuk_coil, c.plunger_coil
  local sc = pinball.scene
  scene.attract, scene.score, scene.over =
    sc.scene_attract, sc.scene_score, sc.scene_game_over
  mode_scene[1], mode_scene[2], mode_scene[3], mode_scene[4], mode_scene[5] =
    sc.scene_banana_shots, sc.scene_backbox_play, sc.scene_xl_ball,
    sc.scene_magnet_compare, sc.scene_glo_ball
  snd.bumper, snd.sling = pinball.sound.sfx_bumper, pinball.sound.sfx_sling
  local d = pinball.dispenser
  disp.feed, disp.magnet, disp.xl, disp.glo = d.feed, d.magnet_feed, d.xl_feed, d.glo_feed
  local pr = pinball.profile
  prof.steel, prof.low_rem = pr.standard_steel, pr.low_rem_steel
  prof.xl, prof.glo = pr.jumbo_xl_resin, pr.uv_globall
  mag = pinball.magnet.hold_magnet
  lamp_mode = pinball.lamp.mode_lamp
  local p = pinball.persist_key
  k_games, k_high, k_ball, k_state, k_sel = p.games_played, p.high_score, p.ball, p.game_state, p.mode_sel
  k_cmp_std, k_cmp_low = p.cmp_release_ms_standard, p.cmp_release_ms_low_rem
  mode_key[1], mode_key[2], mode_key[3], mode_key[4], mode_key[5] =
    p["mode:banana_shots"], p["mode:backbox_play"], p["mode:xl_ball"],
    p["mode:magnet_compare"], p["mode:glo_ball"]
end

function on_init()
  bind_ids()
  ticks = pinball.tick()
  games = pinball.persist_get(k_games)
  high = pinball.persist_get(k_high)
  mode_sel = pinball.random(0, MODES - 1)
  -- Mode start counts continue from the stored values (a fresh store
  -- holds 0, as do the cmp_release_ms_* keys until measured). At most 8
  -- persist writes fit one tick's command queue.
  for i = 1, MODES do starts[i] = pinball.persist_get(mode_key[i]) end
  pinball.persist_set(k_ball, 0)
  pinball.persist_set(k_state, ATTRACT)
  pinball.persist_set(k_sel, mode_sel)
  pinball.show_scene(scene.attract)
end

-- Plain-data snapshot of every rule-state local (reload contract).
function on_save_state()
  return {
    state = state, score = score, ball = ball, games = games, high = high,
    ticks = ticks, mode_sel = mode_sel, last_spawn = last_spawn,
    vuk_at = vuk_at, attract_at = attract_at, last_flip = last_flip, last_entry = last_entry,
    s1 = started[1], s2 = started[2], s3 = started[3], s4 = started[4], s5 = started[5],
    n1 = starts[1], n2 = starts[2], n3 = starts[3], n4 = starts[4], n5 = starts[5],
    q1 = shot_next[1], q2 = shot_next[2], q3 = shot_next[3], q4 = shot_next[4], q5 = shot_next[5],
    t1 = shot_start[1], t2 = shot_start[2], t3 = shot_start[3], t4 = shot_start[4], t5 = shot_start[5],
    banana_on = banana_on, backbox_on = backbox_on, xl_on = xl_on, glo_on = glo_on,
    xl_end = xl_end, glo_end = glo_end, glo_launch = glo_launch,
    cmp_phase = cmp_phase, cmp_which = cmp_which, cmp_deadline = cmp_deadline,
    cmp_hold_until = cmp_hold_until, cmp_off = cmp_off,
  }
end

function on_restore_state(v)
  bind_ids()
  if type(v) ~= "table" then return end
  state, score, ball, games, high = v.state, v.score, v.ball, v.games, v.high
  ticks, mode_sel, last_spawn = v.ticks, v.mode_sel, v.last_spawn
  vuk_at, attract_at = v.vuk_at, v.attract_at
  last_flip, last_entry = v.last_flip, v.last_entry
  started[1], started[2], started[3], started[4], started[5] = v.s1, v.s2, v.s3, v.s4, v.s5
  starts[1], starts[2], starts[3], starts[4], starts[5] = v.n1, v.n2, v.n3, v.n4, v.n5
  shot_next[1], shot_next[2], shot_next[3], shot_next[4], shot_next[5] = v.q1, v.q2, v.q3, v.q4, v.q5
  shot_start[1], shot_start[2], shot_start[3], shot_start[4], shot_start[5] = v.t1, v.t2, v.t3, v.t4, v.t5
  banana_on, backbox_on, xl_on, glo_on = v.banana_on, v.backbox_on, v.xl_on, v.glo_on
  xl_end, glo_end, glo_launch = v.xl_end, v.glo_end, v.glo_launch
  cmp_phase, cmp_which, cmp_deadline = v.cmp_phase, v.cmp_which, v.cmp_deadline
  cmp_hold_until, cmp_off = v.cmp_hold_until, v.cmp_off
  -- The new state redraws the score on its first tick.
  dirty = true
end

local function add(points)
  score = score + points
  dirty = true
end

local function spawn(dispenser, profile)
  pinball.spawn_ball(dispenser, profile)
  last_spawn = ticks
end

local function set_sel(v)
  mode_sel = v % MODES
  pinball.persist_set(k_sel, mode_sel)
end

-- Ends every running mode (the player's ball has ended). Balls already in
-- play keep playing and drain on their own.
local function end_modes()
  banana_on, backbox_on, xl_on, glo_on = false, false, false, false
  xl_end, glo_end, glo_launch = -1, -1, -1
  if cmp_phase ~= CMP_IDLE then
    cmp_phase = CMP_IDLE
    pinball.set_magnet(mag, 0)
  end
  for i = 1, MODES do
    started[i] = false
    shot_next[i] = 0
  end
  pinball.set_lamp(lamp_mode, 0)
end

local function serve_ball()
  set_sel(mode_sel + 1)
  pinball.persist_set(k_ball, ball)
  spawn(disp.feed, prof.steel)
end

local function start_game()
  state = GAME
  score, ball = 0, 1
  attract_at = -1
  dirty = true
  end_modes()
  pinball.persist_set(k_state, GAME)
  pinball.show_scene(scene.score)
  serve_ball()
end

local function end_ball()
  end_modes()
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

-- Magnet comparison: serve the next ball onto the energised magnet.
local function cmp_serve()
  pinball.set_magnet(mag, 1)
  if cmp_which == 0 then
    spawn(disp.magnet, prof.steel)
  else
    spawn(disp.magnet, prof.low_rem)
  end
  cmp_phase = CMP_WAIT
  cmp_deadline = ticks + CMP_TIMEOUT
end

-- The current comparison ball is done (measured or timed out).
local function cmp_next()
  pinball.set_magnet(mag, 0)
  if cmp_which == 0 then
    cmp_which = 1
    cmp_phase = CMP_GAP
    cmp_deadline = ticks + CMP_GAP_TICKS
  else
    cmp_phase = CMP_IDLE
  end
end

local function start_mode(i)
  started[i] = true
  starts[i] = starts[i] + 1
  pinball.persist_set(mode_key[i], starts[i])
  pinball.show_scene(mode_scene[i])
  pinball.set_lamp(lamp_mode, 1)
  add(MODE_START_POINTS)
  if i == 1 then
    banana_on = true
  elseif i == 2 then
    backbox_on = true
  elseif i == 3 then
    xl_on = true
    xl_end = ticks + XL_MODE_TICKS
    spawn(disp.xl, prof.xl)
  elseif i == 4 then
    if cmp_phase == CMP_IDLE then
      cmp_which = 0
      cmp_serve()
    end
  else
    glo_on = true
    glo_end = ticks + GLO_MODE_TICKS
    glo_launch = ticks + GLO_LAUNCH_DELAY
    spawn(disp.glo, prof.glo)
  end
end

-- A qualifying shot `i` was made.
local function shot_made(i)
  if i == 1 and banana_on then add(25000) end
  if state == GAME and mode_sel == i - 1 and not started[i] then
    start_mode(i)
  end
end

-- Shot tracking on a closing edge of switch `id`.
local function shot_edge(id)
  for i = 1, MODES do
    if shot_next[i] > 0 and ticks - shot_start[i] > shot_win[i] then
      shot_next[i] = 0
    end
    if shot_next[i] == 1 and id == shot_b[i] then
      shot_next[i] = 0
      shot_made(i)
    elseif id == shot_a[i] then
      shot_next[i] = 1
      shot_start[i] = ticks
    end
  end
end

function on_switch(id, closed)
  bind_ids()
  ticks = pinball.tick()
  if not closed then
    if id == sw.ll_eos or id == sw.lr_eos then
      last_flip = ticks
    end
    if id == sw.magnet and cmp_phase == CMP_RELEASE then
      local ms = ticks - cmp_off
      if cmp_which == 0 then
        pinball.persist_set(k_cmp_std, ms)
      else
        pinball.persist_set(k_cmp_low, ms)
      end
      if state == GAME then add(10000) end
      cmp_next()
    end
    return
  end
  if id == sw.start then
    if state ~= GAME then start_game() end
    return
  end
  -- The backbox VUK ejects whatever it holds, in every state.
  if id == sw.bb_vuk then
    vuk_at = ticks + VUK_HOLD
  end
  if id == sw.magnet and cmp_phase == CMP_WAIT then
    cmp_phase = CMP_HOLD
    cmp_hold_until = ticks + CMP_HOLD_TICKS
  end
  if id == sw.xl_trough then
    xl_on = false
    xl_end = -1
  end
  if state ~= GAME then return end
  if id == sw.action then
    set_sel(mode_sel + 1)
    return
  end
  shot_edge(id)
  if (id == sw.orbit_entry or id == sw.vuk_lane or id == sw.magnet
      or id == sw.b1 or id == sw.b2 or id == sw.b3)
      and ticks - last_flip <= FLIP_SHOT_TICKS then
    -- One bonus per flip.
    last_flip = -1000000
    add(FLIP_SHOT_POINTS)
  end
  if id == sw.launch then
    pinball.pulse_coil(coil.plunger, 300)
  elseif id == sw.b1 or id == sw.b2 or id == sw.b3 then
    if id == sw.b1 then
      pinball.pulse_coil(coil.b1, 20)
    elseif id == sw.b2 then
      pinball.pulse_coil(coil.b2, 20)
    else
      pinball.pulse_coil(coil.b3, 20)
    end
    pinball.play_sound(snd.bumper, 1)
    if glo_on then add(20) else add(10) end
  elseif id == sw.sl then
    pinball.pulse_coil(coil.sl, 25); pinball.play_sound(snd.sling, 1); add(10)
  elseif id == sw.orbit_entry then
    last_entry = ticks
    add(100)
  elseif id == sw.orbit_top then
    if ticks - last_entry <= ORBIT_TICKS then add(ORBIT_POINTS) else add(500) end
  elseif id == sw.vuk_lane then
    add(100)
  elseif id == sw.bb_vuk then
    add(500)
  elseif id == sw.bb_eos then
    if backbox_on then add(1000) end
  elseif id == sw.bb_drain then
    if backbox_on then add(5000) else add(500) end
  elseif id == sw.magnet then
    add(100)
  elseif id == sw.xl_lane then
    add(500)
  end
end

function on_tick()
  bind_ids()
  ticks = pinball.tick()
  if vuk_at >= 0 and ticks >= vuk_at then
    vuk_at = -1
    pinball.pulse_coil(coil.vuk, 20)
  end
  if cmp_phase == CMP_WAIT and ticks > cmp_deadline then
    cmp_next()
  elseif cmp_phase == CMP_HOLD and ticks >= cmp_hold_until then
    pinball.set_magnet(mag, 0)
    cmp_off = ticks
    cmp_phase = CMP_RELEASE
    cmp_deadline = ticks + CMP_TIMEOUT
  elseif cmp_phase == CMP_RELEASE and ticks > cmp_deadline then
    cmp_next()
  elseif cmp_phase == CMP_GAP and ticks >= cmp_deadline then
    cmp_serve()
  end
  if xl_end >= 0 and ticks >= xl_end then
    xl_on = false
    xl_end = -1
  end
  if glo_launch >= 0 and ticks >= glo_launch then
    glo_launch = -1
    pinball.pulse_coil(coil.plunger, 300)
  end
  if glo_end >= 0 and ticks >= glo_end then
    glo_on = false
    glo_end = -1
  end
  -- A ball ends when no ball is left in play (every ball leaves through a
  -- trough switch); a ball served this or the last tick is not counted yet.
  if state == GAME and pinball.balls_in_play() == 0 and ticks - last_spawn > 1 then
    end_ball()
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
