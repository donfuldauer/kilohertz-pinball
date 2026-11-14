-- Spec parity ruleset (subphase 4.5), Lua side. Integer-only rules: float
-- arguments are exact dyadic constants. parity_rules.wat is the same ruleset
-- written by hand against the Wasm ABI; both must produce identical command
-- queues on every callback.
local P = pinball
local C_EJECT, C_SLING = P.coil.trough_eject, P.coil.left_sling
local C_KNOCK, C_GATE = P.coil.knocker, P.coil.hold_gate
local L_AGAIN, L_JACK = P.lamp.shoot_again, P.lamp.jackpot
local M_CENTER = P.magnet.center
local DV_GATE = P.diverter.ramp_gate
local F_UPPER = P.flipper.upper
local D_TROUGH = P.dispenser.trough
local PR_STEEL, PR_CERAMIC = P.profile.steel, P.profile.ceramic
local S_BELL, S_DRAIN = P.sound.bell, P.sound.drain
local SC_ATTRACT, SC_MULTI = P.scene.attract, P.scene.multiball
local K_HS, K_CREDITS, K_LAST = P.persist_key.high_score_1, P.persist_key.credits, P.persist_key.last
local SW_START, SW_LSLING = P.sw.start, P.sw.left_sling
local SW_TROUGH, SW_SPINNER = P.sw.trough_1, P.sw.spinner

local n, score, gate, spins = 0, 0, 0, 0

function on_init()
  P.show_scene(SC_ATTRACT)
  P.set_gi_tint(1, 0.5, 0.25, 1)
  P.set_score(1, 0)
  P.persist_set(K_CREDITS, P.persist_get(K_CREDITS) + 1)
  P.set_lamp(L_AGAIN, 1)
  score = P.random(1, 6) * 100
  P.set_score(2, score)
end

function on_tick()
  n = n + 1
  if n % 7 == 0 then
    local r = P.random(1, 6)
    if r % 2 == 1 then P.set_lamp(r, 1) else P.set_lamp(r, 0.5) end
  end
  if n % 13 == 0 then
    gate = 1 - gate
    P.hold_coil(C_GATE, gate == 1)
  end
  if n % 50 == 0 then
    local r = P.random(0, 999)
    score = score + r
    P.set_score(1 + (n // 50) % 4, score)
  end
  if n % 97 == 0 then
    local r = P.random(-5, 5)
    if r < 0 then P.play_sound(S_DRAIN, 0.25) else P.play_sound(S_BELL, 1) end
    score = score + r
  end
  if P.tick() % 250 == 0 then
    P.persist_set(K_LAST, P.persist_get(K_LAST) + P.balls_in_play() + P.tick())
  end
  if P.switch(SW_SPINNER) then
    spins = spins + 1
    P.set_lamp(L_JACK, 1)
    if spins % 4 == 0 then P.set_magnet(M_CENTER, 0.5) end
  elseif n % 100 == 0 then
    P.set_magnet(M_CENTER, 0)
  end
  if n % 333 == 0 then
    local q = n // 333
    if q % 2 == 0 then P.spawn_ball(D_TROUGH, PR_STEEL) else P.spawn_ball(D_TROUGH, PR_CERAMIC) end
    P.set_diverter(DV_GATE, q % 2 == 1)
    P.set_zipper(F_UPPER, q % 3 == 0)
    P.show_scene(SC_MULTI)
  end
  if n % 211 == 0 then
    local big = P.random(math.mininteger, math.maxinteger)
    score = score + (big % 1000) -- floored modulo: always 0..999
    P.persist_set(K_HS, P.random(0, 1 << 40))
    P.set_score(4, score)
  end
  if n % 500 == 0 then
    for i = 0, 39 do P.pulse_coil(C_KNOCK, i) end -- 32 fit, 8 are dropped
  end
end

function on_switch(id, closed)
  if closed then
    P.pulse_coil(id % 8, 10 + id)
    score = score + id * 10
    P.set_score(1, score)
    if id == SW_LSLING then
      local r = P.random(1, 3)
      P.pulse_coil(C_SLING, 20 + r)
      P.play_sound(S_BELL, 0.5)
    elseif id == SW_START then
      P.spawn_ball(D_TROUGH, PR_STEEL)
      P.pulse_coil(C_EJECT, 30)
      P.persist_set(K_CREDITS, P.persist_get(K_CREDITS) - 1)
    end
  else
    P.set_lamp(id, 0)
    if id == SW_TROUGH and P.balls_in_play() == 0 then
      P.show_scene(SC_ATTRACT)
      if score > P.persist_get(K_HS) then P.persist_set(K_HS, score) end
    end
  end
end
