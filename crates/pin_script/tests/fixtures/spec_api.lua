-- Spec fixture: one test case per switch id, dispatched from on_switch.
-- Every case uses the documented pinball.* API only.
local p = pinball
local cases = {}

cases[1] = function()
  p.pulse_coil(p.coil.left_sling, 30)
  p.pulse_coil(31, 65535)
  p.pulse_coil(0, 1)
end

cases[2] = function()
  p.hold_coil(p.coil.left_sling, true)
  p.hold_coil(1, false)
end

cases[3] = function()
  p.set_magnet(p.magnet.center, 0.5)
  p.set_magnet(4, 0)
  p.set_magnet(7, 1)
end

cases[4] = function()
  p.set_zipper(p.flipper.upper, true)
  p.set_zipper(0, false)
end

cases[5] = function()
  p.set_diverter(p.diverter.ramp_gate, true)
  p.set_diverter(5, false)
end

cases[6] = function()
  p.spawn_ball(p.dispenser.trough, p.profile.ceramic)
  p.spawn_ball(9, 0)
end

cases[7] = function()
  p.set_lamp(p.lamp.jackpot, 0.25)
  p.set_lamp(2, 1)
  p.set_lamp(255, 0)
end

cases[8] = function()
  p.set_gi_tint(0.25, 0.5, 0.75, 1.0)
end

cases[9] = function()
  p.set_score(1, 1000)
  p.set_score(4, 9007199254740993) -- 2^53 + 1: must not round-trip through a double
  p.set_score(2, 0)
end

cases[10] = function()
  p.show_scene(p.scene.multiball)
  p.show_scene(14)
end

cases[11] = function()
  p.play_sound(p.sound.bell, 0.5)
  p.play_sound(13, 1.0)
end

cases[12] = function()
  p.persist_set(p.persist_key.credits, -5)
  p.persist_set(63, math.maxinteger)
  p.persist_set(0, math.mininteger)
end

-- Host view reads, reported back through persist writes and lamps.
cases[13] = function()
  p.persist_set(0, p.persist_get(p.persist_key.credits))
  p.persist_set(1, p.tick())
  p.persist_set(2, p.balls_in_play())
  p.persist_set(3, p.persist_get(63))
  p.persist_set(4, p.persist_get(0))
  local function b(v) return v and 1 or 0 end
  p.set_lamp(0, b(p.switch(p.sw.left_sling)))
  p.set_lamp(1, b(p.switch(p.sw.right_sling)))
  p.set_lamp(2, b(p.switch(255)))
  p.set_lamp(3, b(p.switch(63)))
  p.set_lamp(4, b(p.switch(64)))
  p.set_lamp(5, b(p.switch(0)))
  p.set_lamp(10, b(math.type(p.tick()) == "integer"))
  p.set_lamp(11, b(type(p.switch(7)) == "boolean"))
  p.set_lamp(12, b(math.type(p.balls_in_play()) == "integer"))
  p.set_lamp(13, b(math.type(p.persist_get(0)) == "integer"))
end

-- Name tables: every name maps to its integer id.
cases[14] = function()
  local function check(lamp, tbl, expected)
    local ok = type(tbl) == "table" or type(tbl) == "userdata"
    if ok then
      for name, id in pairs(expected) do
        if tbl[name] ~= id or math.type(tbl[name]) ~= "integer" then ok = false end
      end
    end
    p.set_lamp(lamp, ok and 1 or 0)
  end
  check(0, p.sw, { start = 0, left_sling = 7, right_sling = 8, trough_1 = 20, spinner = 63, far = 255 })
  check(1, p.coil, { trough_eject = 1, left_sling = 3, knocker = 31 })
  check(2, p.lamp, { shoot_again = 2, jackpot = 200 })
  check(3, p.magnet, { center = 4 })
  check(4, p.diverter, { ramp_gate = 5 })
  check(5, p.flipper, { left = 0, right = 1, upper = 6 })
  check(6, p.dispenser, { trough = 9 })
  check(7, p.profile, { steel = 0, ceramic = 11 })
  check(8, p.sound, { bell = 12, drain = 13 })
  check(9, p.scene, { attract = 14, multiball = 15 })
  check(10, p.persist_key, { high_score_1 = 0, credits = 16, last = 63 })
end

cases[15] = function() end

-- Every documented function exists and is callable.
cases[16] = function()
  local fns = {
    "pulse_coil", "hold_coil", "set_magnet", "set_lamp", "set_gi_tint", "set_score",
    "spawn_ball", "set_diverter", "set_zipper", "show_scene", "play_sound", "switch",
    "tick", "balls_in_play", "random", "persist_get", "persist_set",
  }
  local missing = 0
  for _, name in ipairs(fns) do
    if type(p[name]) ~= "function" then missing = missing + 1 end
  end
  p.persist_set(0, missing)
  p.persist_set(1, #fns)
end

function on_switch(id, closed)
  local case = cases[id]
  if case then case() end
end
