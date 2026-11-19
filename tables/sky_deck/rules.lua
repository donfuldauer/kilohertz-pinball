-- Sky Deck rules (Subphase 6.2, variant B). Integer arithmetic only (the
-- UV tint's 0..1 components are constants); the callbacks create no Lua
-- objects (no strings, tables or closures), so the steady state performs no
-- Lua allocation.
--
-- Game flow: attract -> START -> a 3-ball game -> game over -> attract.
-- Each ball is a steel ball served by dispenser `feed` into the shooter
-- lane. The rules count the balls on the table (`live`: serves and Glo-Ball
-- spawns minus trough drains); a ball of the game ends only when every ball
-- on the table has drained into trough_1 / trough_2.
--
-- Sky Bridge: during normal play (game running, Night Overdrive not active
-- and not armed) an `action` press opens the Sky Bridge Diverter
-- (`set_diverter(bridge_diverter, true)` and its blade coil held) for
-- exactly 8000 ticks (8.0 s), then closes it. A press while it is open is
-- ignored (no extension). Persist `bridge_open` is 1 while it is open.
--
-- Night Overdrive ("Reactor Core" completion): armed (persist
-- `overdrive_ready` = 1) once, in the current ball, the Newton ball has been
-- struck and all three drop targets are down. It starts on the first of an
-- `action` press (while armed it replaces the bridge toggle) or the next
-- `vuk_sw` closure: persist `mode:night_overdrive` +1, scene
-- scene_night_overdrive, a UV GI tint and a Glo-Ball (uv_globall) from
-- dispenser `glo_feed`, which rolls into the centre VUK and is lifted onto
-- the upper deck (the VUK holds a ball 4 s, but only 0.5 s from the mode
-- start until the Glo-Ball is lifted, so it reaches the deck within about
-- 2 s). While it runs, upper-deck scoring (deck_opto, drop
-- targets, bank bonus) is doubled. It ends at the first trough drain while
-- it runs (the multiball is over) or when the ball ends; the GI tint is
-- restored to (1, 1, 1, 1) then and at game over. At most one per ball.
--
-- Scoring (player 1): sling 1000, loop entry 1000, loop top 1000 plus a
-- loop shot bonus of 50000 when it follows loop_entry within 3 s (the shot
-- up the left leg; a plunge rides the loop the other way), ramp entry 1000,
-- ramp made 25000 (once per pass), VUK 100, upper deck 100, Newton ball
-- 100, drop target 100, drop bank complete 1000. A tilt voids
-- scoring until the ball ends.
--
-- Persistent values (`[persist]` keys, readable by the host):
--   games_played, high_score, ball (ball in play, 0 when no game),
--   game_state (0 attract, 1 game, 2 game over),
--   mode:night_overdrive (Night Overdrive starts; the 2.9 soak's signal),
--   bridge_open (1 while the Sky Bridge is open), overdrive_ready (1 armed).
--
-- Hot reload: a reload runs this file in a fresh state and does not call
-- on_init again; the old on_save_state() result is handed to the new
-- on_restore_state(). The id tables are therefore bound lazily (bind_ids,
-- name lookups only, no actions) and every rule-state local round-trips
-- through on_save_state / on_restore_state as plain data.

local ATTRACT, GAME, OVER = 0, 1, 2
local BALLS_PER_GAME = 3
local VUK_HOLD = 4000
-- The VUK's hold while the Night Overdrive Glo-Ball is on its way to the
-- upper deck (from the mode start until the eject that follows its
-- dispense): the steel ball and the Glo-Ball are lifted at once.
local VUK_QUICK = 500
local DROP_RESET_DELAY = 1500
local GAME_OVER_TICKS = 4000
local BRIDGE_TICKS = 8000
-- Ticks after a VUK eject before the Glo-Ball is dispensed into the pocket
-- (the scoop is free again by then).
local GLO_DELAY = 300
-- A second ramp_made closing within this many ticks is the same pass (the
-- ball rolling back down past the opto).
local RAMP_DEBOUNCE = 1500
-- loop_top within this many ticks of loop_entry is a loop shot.
local LOOP_WINDOW = 3000
-- How often the safety checks run (VUK switch held without an eject
-- pending; no ball left on the table while the rules still count one).
local CHECK_EVERY = 256
-- UV GI tint (r, g, b, intensity), each 0..1.
local UV_R, UV_G, UV_B, UV_I = 0.35, 0.15, 1.0, 0.8

local state = ATTRACT
local score, ball, games, high = 0, 0, 0, 0
local ticks = 0
local dirty = true
local live = 0
local last_drain = -1
local tilted = false
local vuk_at, drop_at, attract_at = -1, -1, -1
local bridge_until = -1
local last_ramp_made = -1000000
local last_loop_entry = -1000000
local newton_hit = false
local drops = 0 -- bit 1, 2, 4: drop_1..3 down
local drops_done = false
local od_ready, od_active, od_used = false, false, false
local od_starts = 0
local glo_at = -1
local glo_after_eject = false
-- Night Overdrive started and its Glo-Ball not lifted yet (VUK_QUICK holds).
local glo_rising = false
-- The Glo-Ball has been dispensed (the next VUK eject lifts it).
local glo_out = false
local empty_checks = 0

local sw, coil, scene, snd = {}, {}, {}, {}
local disp_feed, disp_glo, prof_steel, prof_glo, diverter
local lamp_od, lamp_bridge
local k_games, k_high, k_ball, k_state, k_mode, k_bridge, k_ready

-- Fills the id tables from the read-only `pinball` name tables. Performs no
-- action (no scene, persist write, coil or sound), so it is safe from the
-- reload hooks. Cheap no-op once bound.
local function bind_ids()
  if sw.start ~= nil then return end
  local s = pinball.sw
  sw.start, sw.launch, sw.tilt, sw.action = s.start, s.launch, s.tilt, s.action
  sw.trough_1, sw.trough_2 = s.trough_1, s.trough_2
  sw.sl, sw.sr = s.sling_left_sw, s.sling_right_sw
  sw.loop_entry, sw.loop_top = s.loop_entry, s.loop_top
  sw.ramp_entry, sw.ramp_made = s.ramp_entry, s.ramp_made
  sw.vuk, sw.deck = s.vuk_sw, s.deck_opto
  sw.newton = s.newton_sw
  sw.d1, sw.d2, sw.d3 = s.drop_1_sw, s.drop_2_sw, s.drop_3_sw
  local c = pinball.coil
  coil.sl, coil.sr = c.sling_left_coil, c.sling_right_coil
  coil.vuk, coil.plunger, coil.drop = c.vuk_coil, c.plunger_coil, c.drop_reset_coil
  coil.bridge = c.bridge_coil
  scene.attract = pinball.scene.scene_attract
  scene.score = pinball.scene.scene_score
  scene.over = pinball.scene.scene_game_over
  scene.od = pinball.scene.scene_night_overdrive
  snd.bumper, snd.sling = pinball.sound.sfx_bumper, pinball.sound.sfx_sling
  disp_feed, disp_glo = pinball.dispenser.feed, pinball.dispenser.glo_feed
  prof_steel = pinball.profile.standard_steel
  prof_glo = pinball.profile.uv_globall
  diverter = pinball.diverter.bridge_diverter
  lamp_od, lamp_bridge = pinball.lamp.overdrive_lamp, pinball.lamp.bridge_lamp
  local p = pinball.persist_key
  k_games, k_high, k_ball, k_state = p.games_played, p.high_score, p.ball, p.game_state
  k_mode, k_bridge, k_ready = p["mode:night_overdrive"], p.bridge_open, p.overdrive_ready
end

function on_init()
  bind_ids()
  games = pinball.persist_get(k_games)
  high = pinball.persist_get(k_high)
  pinball.persist_set(k_ball, 0)
  pinball.persist_set(k_state, ATTRACT)
  pinball.persist_set(k_mode, 0)
  pinball.persist_set(k_bridge, 0)
  pinball.persist_set(k_ready, 0)
  pinball.set_gi_tint(1, 1, 1, 1)
  pinball.show_scene(scene.attract)
end

-- Plain-data snapshot of every rule-state local (reload contract).
function on_save_state()
  return {
    state = state, score = score, ball = ball, games = games, high = high,
    ticks = ticks, live = live, last_drain = last_drain, tilted = tilted,
    vuk_at = vuk_at, drop_at = drop_at, attract_at = attract_at,
    bridge_until = bridge_until, last_ramp_made = last_ramp_made,
    last_loop_entry = last_loop_entry,
    newton_hit = newton_hit, drops = drops, drops_done = drops_done,
    od_ready = od_ready, od_active = od_active, od_used = od_used,
    od_starts = od_starts, glo_at = glo_at, glo_after_eject = glo_after_eject,
    glo_rising = glo_rising, glo_out = glo_out,
    empty_checks = empty_checks,
  }
end

function on_restore_state(v)
  bind_ids()
  if type(v) ~= "table" then return end
  state, score, ball, games, high = v.state, v.score, v.ball, v.games, v.high
  ticks, live, last_drain, tilted = v.ticks, v.live, v.last_drain, v.tilted
  vuk_at, drop_at, attract_at = v.vuk_at, v.drop_at, v.attract_at
  bridge_until, last_ramp_made = v.bridge_until, v.last_ramp_made
  last_loop_entry = v.last_loop_entry
  newton_hit, drops, drops_done = v.newton_hit, v.drops, v.drops_done
  od_ready, od_active, od_used = v.od_ready, v.od_active, v.od_used
  od_starts, glo_at, glo_after_eject = v.od_starts, v.glo_at, v.glo_after_eject
  glo_rising, glo_out = v.glo_rising, v.glo_out
  empty_checks = v.empty_checks
  -- The new state redraws the score on its first tick.
  dirty = true
end

local function add(points)
  if tilted then return end
  score = score + points
  dirty = true
end

-- Upper-deck points: doubled while Night Overdrive runs.
local function add_deck(points)
  if od_active then points = points * 2 end
  add(points)
end

local function serve_ball()
  pinball.spawn_ball(disp_feed, prof_steel)
  live = 1
  empty_checks = 0
  pinball.persist_set(k_ball, ball)
end

local function close_bridge()
  if bridge_until < 0 then return end
  bridge_until = -1
  pinball.set_diverter(diverter, false)
  pinball.hold_coil(coil.bridge, false)
  pinball.set_lamp(lamp_bridge, 0)
  pinball.persist_set(k_bridge, 0)
end

local function open_bridge()
  bridge_until = ticks + BRIDGE_TICKS
  pinball.set_diverter(diverter, true)
  pinball.hold_coil(coil.bridge, true)
  pinball.set_lamp(lamp_bridge, 1)
  pinball.persist_set(k_bridge, 1)
end

local function set_ready(on)
  if od_ready == on then return end
  od_ready = on
  if on then
    pinball.persist_set(k_ready, 1)
    pinball.set_lamp(lamp_od, 1)
  else
    pinball.persist_set(k_ready, 0)
    if not od_active then pinball.set_lamp(lamp_od, 0) end
  end
end

local function end_overdrive()
  if not od_active then return end
  od_active = false
  pinball.set_gi_tint(1, 1, 1, 1)
  pinball.set_lamp(lamp_od, 0)
  pinball.show_scene(scene.score)
end

local function start_overdrive()
  set_ready(false)
  od_active = true
  od_used = true
  od_starts = od_starts + 1
  pinball.persist_set(k_mode, od_starts)
  pinball.set_gi_tint(UV_R, UV_G, UV_B, UV_I)
  pinball.set_lamp(lamp_od, 1)
  pinball.show_scene(scene.od)
  -- The Glo-Ball counts as on the table from now on; it is dispensed into
  -- the VUK pocket once the scoop is free.
  live = live + 1
  glo_rising, glo_out = true, false
  if vuk_at >= 0 then
    -- A ball is in the scoop: lift it now, then dispense the Glo-Ball.
    glo_after_eject = true
    if vuk_at > ticks + VUK_QUICK then vuk_at = ticks + VUK_QUICK end
  else
    glo_at = ticks
  end
end

-- Ticks the VUK holds a ball captured now.
local function vuk_hold()
  if glo_rising then return VUK_QUICK end
  return VUK_HOLD
end

local function check_armed()
  if newton_hit and drops_done and not od_used and not od_active then
    set_ready(true)
  end
end

local function reset_bank()
  drops = 0
  drop_at = -1
  pinball.pulse_coil(coil.drop, 150)
end

local function start_game()
  state = GAME
  score, ball = 0, 1
  tilted = false
  attract_at = -1
  dirty = true
  pinball.persist_set(k_state, GAME)
  pinball.show_scene(scene.score)
  serve_ball()
end

local function end_ball()
  end_overdrive()
  set_ready(false)
  glo_at, glo_after_eject = -1, false
  glo_rising, glo_out = false, false
  newton_hit, drops_done, od_used, tilted = false, false, false, false
  if drops ~= 0 or drop_at >= 0 then reset_bank() end
  live = 0
  if ball < BALLS_PER_GAME then
    ball = ball + 1
    serve_ball()
    return
  end
  state = OVER
  ball = 0
  games = games + 1
  if score > high then high = score end
  close_bridge()
  pinball.set_gi_tint(1, 1, 1, 1)
  pinball.persist_set(k_games, games)
  pinball.persist_set(k_high, high)
  pinball.persist_set(k_ball, 0)
  pinball.persist_set(k_state, OVER)
  pinball.show_scene(scene.over)
  attract_at = ticks + GAME_OVER_TICKS
end

local function drained()
  -- One drain can close both trough switches in the same tick.
  if last_drain == ticks then return end
  last_drain = ticks
  live = live - 1
  -- The first drain of the Night Overdrive multiball ends the mode.
  end_overdrive()
  if live > 0 and glo_at < 0 and not glo_after_eject
      and pinball.balls_in_play() == 0 then
    -- Two balls drained in one tick: nothing is left on the table.
    live = 0
  end
  if live <= 0 then end_ball() end
end

local function drop_down(bit)
  if drops & bit ~= 0 then return end
  drops = drops | bit
  add_deck(100)
  if drops == 7 then
    add_deck(1000)
    drops_done = true
    drop_at = ticks + DROP_RESET_DELAY
    check_armed()
  end
end

function on_switch(id, closed)
  bind_ids()
  ticks = pinball.tick()
  if not closed then return end
  if id == sw.start then
    if state ~= GAME then start_game() end
    return
  end
  if state ~= GAME then return end
  if id == sw.trough_1 or id == sw.trough_2 then
    drained()
  elseif id == sw.launch then
    pinball.pulse_coil(coil.plunger, 300)
  elseif id == sw.action then
    if od_ready and not od_active then
      start_overdrive()
    elseif not od_active and not od_ready and bridge_until < 0 then
      open_bridge()
    end
  elseif id == sw.tilt then
    tilted = true
  elseif id == sw.sl then
    pinball.pulse_coil(coil.sl, 25); pinball.play_sound(snd.sling, 1); add(1000)
  elseif id == sw.sr then
    pinball.pulse_coil(coil.sr, 25); pinball.play_sound(snd.sling, 1); add(1000)
  elseif id == sw.loop_entry then
    add(1000)
    last_loop_entry = ticks
  elseif id == sw.loop_top then
    add(1000)
    if ticks - last_loop_entry <= LOOP_WINDOW then add(50000) end
    last_loop_entry = -1000000
  elseif id == sw.ramp_entry then
    add(1000)
  elseif id == sw.ramp_made then
    if ticks - last_ramp_made > RAMP_DEBOUNCE then add(25000) end
    last_ramp_made = ticks
  elseif id == sw.vuk then
    add(100)
    pinball.play_sound(snd.bumper, 1)
    if vuk_at < 0 then vuk_at = ticks + vuk_hold() end
    if od_ready and not od_active then start_overdrive() end
  elseif id == sw.deck then
    add_deck(100)
  elseif id == sw.newton then
    add(100)
    newton_hit = true
    check_armed()
  elseif id == sw.d1 then
    drop_down(1)
  elseif id == sw.d2 then
    drop_down(2)
  elseif id == sw.d3 then
    drop_down(4)
  end
end

function on_tick()
  bind_ids()
  ticks = pinball.tick()
  if vuk_at >= 0 and ticks >= vuk_at then
    vuk_at = -1
    pinball.pulse_coil(coil.vuk, 20)
    if glo_out then glo_rising, glo_out = false, false end
    if glo_after_eject then
      glo_after_eject = false
      glo_at = ticks + GLO_DELAY
    end
  end
  if glo_at >= 0 and ticks >= glo_at then
    glo_at = -1
    pinball.spawn_ball(disp_glo, prof_glo)
    glo_out = true
  end
  if drop_at >= 0 and ticks >= drop_at then
    reset_bank()
  end
  if bridge_until >= 0 and ticks >= bridge_until then
    close_bridge()
  end
  if attract_at >= 0 and ticks >= attract_at then
    attract_at = -1
    if state == OVER then
      state = ATTRACT
      pinball.persist_set(k_state, ATTRACT)
      pinball.show_scene(scene.attract)
    end
  end
  if ticks % CHECK_EVERY == 0 and state == GAME then
    -- A ball held in the scoop with no eject pending (its closing edge was
    -- lost in an eject tick): eject it.
    if vuk_at < 0 and pinball.switch(sw.vuk) then vuk_at = ticks + vuk_hold() end
    -- Nothing on the table for two checks in a row while the rules still
    -- count a ball (a ball that left the table without draining): end it.
    if live > 0 and glo_at < 0 and not glo_after_eject
        and pinball.balls_in_play() == 0 then
      empty_checks = empty_checks + 1
      if empty_checks >= 2 then
        live = 0
        end_ball()
      end
    else
      empty_checks = 0
    end
  end
  if dirty then
    pinball.set_score(1, score)
    dirty = false
  end
end
